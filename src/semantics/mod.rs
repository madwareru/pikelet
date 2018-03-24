//! The semantics of the language
//!
//! We take a bidirectional approach to type checking, splitting it into two
//! phases: type checking and type inference. This makes the flow of information
//! through the type checker clear and relatively easy to reason about.
//!
//! # Judgements
//!
//! The semantics of Pikelet are described by the following judgement forms:
//!
//! | name           | notation         | inputs        | outputs  | implementing function  |
//! |----------------|------------------|---------------|----------|------------------------|
//! | normalization  | `Γ ⊢ t ⇒ v`     | `Γ`, `t`      | `v`      | `semantics::normalize` |
//! | type checking  | `Γ ⊢ r ↑ V ⤳ t` | `Γ`, `r`, `V` | `t`      | `semantics::check`     |
//! | type inference | `Γ ⊢ r ↓ V ⤳ t` | `Γ`, `r`      | `V`, `t` | `semantics::infer`     |
//!
//! The judgements rely on the syntax for terms, values, and contexts that were
//! previously defined in `syntax::core`.  Care has been taken to design the
//! judgments in such a way that they are 'syntax-directed', meaning an
//! algorithm can be clearly derived from them.
//!
//! ## Elaboration
//!
//! Elaboration is the process of filling in missing information that the
//! programmer omitted in the original code, generally based on the results
//! of type inference.
//!
//! In Pikelet's judgement forms the elaborated terms are denoted after the
//! wiggly arrow, ie. `⤳`. At the moment not much is added - only the missing
//! type annotations on function parameters. It's unclear at the moment how
//! bidirectional checking could be extended to support more involved
//! elaboration, for example handling implicit arguments like:
//!
//! ```text
//! id : {a : Type} -> a -> a
//! ```
//!
//! Perhaps we'd have to resort to unification-based inference for that! Not
//! sure how that would work in concert with bidirectional checking, and it
//! would be great to hear any advice if folks have any!
//!
//! ## Error Handling
//!
//! If no judgement matches, we return an appropriate error type. Errors that
//! arise in the `check` and `normalize` functions can be converted into
//! `Diagnostic`s to assist in fixing problems. Errors that arise in the
//! `normalize` function are assumed to be bugs in the implementation,
//! because the type checker should catch these problems before hand.
//!
//! # A note on notation
//!
//! We provide [natural deduction][natural-deduction-wikipedia] judgements to
//! define the semantics of our type system. These can be intimidating to read
//! at first, but are a concise way of describing a logical system. The
//! judgements all follow the same general pattern:
//!
//! ```text
//! 1. premise
//! 2. premise
//!    ...
//! n. premise
//! ────────────────
//!    conclusion
//! ```
//!
//! In contrast to traditional mathematical notation, we number the premises
//! to allow use to mark the relevant parts of the code that determine whether
//! these premises are met.
//!
//! Sometimes you will see judgements that do not have any premises:
//!
//! ```text
//! ────────────────
//!    conclusion
//! ````
//!
//! These judgements can be considered ['axioms'][axiom-wikipedia], ie. they are
//! the base cases of our recursive judgements.
//!
//! [natural-deduction-wikipedia]: https://en.wikipedia.org/wiki/Natural_deduction
//! [axiom-wikipedia]: https://en.wikipedia.org/wiki/Axiom

use codespan::ByteSpan;
use nameless::{self, BoundTerm, Embed, Name, Scope, Var};
use std::rc::Rc;

use syntax::core::{Binder, Context, Definition, Level, Module, Neutral, RawModule, RawTerm, Term,
                   Type, Value};

#[cfg(test)]
mod tests;
mod errors;

pub use self::errors::{InternalError, TypeError};

/// Typecheck and elaborate a module
pub fn check_module(module: &RawModule) -> Result<Module, TypeError> {
    let mut context = Context::new();
    let mut definitions = Vec::with_capacity(module.definitions.len());

    for definition in &module.definitions {
        let name = definition.name.clone();
        let (term, ann) = match *definition.ann {
            // We don't have a type annotation available to us! Instead we will
            // attempt to infer it based on the body of the definition
            RawTerm::Hole(_) => infer(&context, &definition.term)?,
            // We have a type annotation! Elaborate it, then nomalize it, then
            // check that it matches the body of the definition
            _ => {
                let (ann, _) = infer(&context, &definition.ann)?;
                let ann = normalize(&context, &ann)?;
                let elab_term = check(&context, &definition.term, &ann)?;
                (elab_term, ann)
            },
        };

        // Add the definition to the context
        context = context.extend_let(Name::user(name.clone()), ann.clone(), term.clone());

        definitions.push(Definition { name, term, ann })
    }

    Ok(Module {
        name: module.name.clone(),
        definitions,
    })
}

/// Evaluate a term in a context
///
/// Normalizes (evaluates) a core term to its normal form under the assumptions
/// in the context.
///
/// ```text
/// Γ ⊢ t ⇒ v
/// ```
pub fn normalize(context: &Context, term: &Rc<Term>) -> Result<Rc<Value>, InternalError> {
    match **term {
        //  1.  Γ ⊢ t ⇒ v
        // ─────────────────────── (EVAL/ANN)
        //      Γ ⊢ t:T ⇒ v
        Term::Ann(_, ref expr, _) => {
            normalize(context, expr) // 1.
        },

        // ─────────────────── (EVAL/TYPE)
        //  Γ ⊢ Type ⇒ Type
        Term::Universe(_, level) => Ok(Value::Universe(level).into()),

        Term::Constant(_, ref c) => Ok(Value::Constant(c.clone()).into()),

        Term::Var(_, ref var) => match *var {
            Var::Free(ref name) => match context.lookup_binder(name) {
                // Can't reduce further - we are in a pi or let binding!
                // We'll have to hope that these are substituted away later,
                // either in EVAL/APP or INFER/APP. For now we just forward the
                // variable name onward:
                //
                //  1.  λx:V ∈ Γ
                // ───────────────────── (EVAL/VAR-LAM)
                //      Γ ⊢ x ⇒ x
                //
                //  2.  Πx:V ∈ Γ
                // ───────────────────── (EVAL/VAR-PI)
                //      Γ ⊢ x ⇒ x
                Some(&Binder::Lam { .. }) | Some(&Binder::Pi { .. }) => {
                    Ok(Value::from(Neutral::Var(var.clone())).into())
                },

                // We have a value in scope, let's use that!
                //
                //  1.  let x:V = t ∈ Γ
                //  2.  Γ ⊢ t ⇒ v
                // ───────────────────── (EVAL/VAR-LET)
                //      Γ ⊢ x ⇒ v
                Some(&Binder::Let { ref value, .. }) => normalize(context, value),

                None => Err(InternalError::UndefinedName {
                    var_span: term.span(),
                    name: name.clone(),
                }),
            },

            // We should always be substituting bound variables with fresh
            // variables when entering scopes using `unbind`, so if we've
            // encountered one here this is definitely a bug!
            Var::Bound(ref name, index) => Err(InternalError::UnsubstitutedDebruijnIndex {
                span: term.span(),
                name: name.clone(),
                index: index,
            }),
        },

        //  1.  Γ ⊢ T₁ ⇒ V₁
        //  2.  Γ, Πx:V ⊢ T₂ ⇒ V₂
        // ─────────────────────────────────── (EVAL/PI)
        //      Γ ⊢ Πx:T₁.T₂ ⇒ Πx:V₁.V₂
        Term::Pi(_, ref scope) => {
            let ((name, Embed(param_ann)), body) = nameless::unbind(scope.clone());

            let ann = normalize(context, &param_ann)?; // 1.
            let body_context = context.extend_pi(name.clone(), ann.clone());
            let body = normalize(&body_context, &body)?; // 2.

            Ok(Value::Pi(Scope::bind((name, Embed(ann)), body)).into())
        },

        //  1.  Γ ⊢ T ⇒ V
        //  2.  Γ, λx:V ⊢ t ⇒ v
        // ──────────────────────────────── (EVAL/LAM)
        //      Γ ⊢ λx:T.t ⇒ λx:V.v
        Term::Lam(_, ref scope) => {
            let ((name, Embed(param_ann)), body) = nameless::unbind(scope.clone());

            let ann = normalize(context, &param_ann)?; // 1.
            let body_context = context.extend_lam(name.clone(), ann.clone());
            let body = normalize(&body_context, &body)?; // 2.

            Ok(Value::Lam(Scope::bind((name, Embed(ann)), body)).into())
        },

        // Perform [β-reduction](https://en.wikipedia.org/wiki/Lambda_calculus#β-reduction),
        // ie. apply functions to their arguments
        //
        //  1.  Γ ⊢ t₁ ⇒ λx:V₁.v₁
        //  2.  Γ, let x:V₁ = t₂ x ⊢ v₁ ⇒ v₁'
        // ───────────────────────────────────── (EVAL/APP)
        //      Γ ⊢ t₁ t₂ ⇒ v₁'
        Term::App(_, ref fn_expr, ref arg) => {
            let fn_value = normalize(context, fn_expr)?; // 1.

            match *fn_value {
                Value::Lam(ref scope) => {
                    // FIXME: do a local unbind here
                    let ((name, Embed(param_ann)), body) = nameless::unbind(scope.clone());

                    let body_context = context.extend_let(name, param_ann, arg.clone());
                    normalize(&body_context, &Rc::new(Term::from(&*body))) // 2.
                },
                Value::Neutral(ref fn_expr) => {
                    Ok(Value::from(Neutral::App(fn_expr.clone(), arg.clone())).into())
                },
                _ => Err(InternalError::ArgumentAppliedToNonFunction {
                    span: fn_expr.span(),
                }),
            }
        },
    }
}

/// Type checking of terms
///
/// Under the assumptions in the context, check that the given term has
/// the expected type and return its elaborated form.
///
/// ```text
/// Γ ⊢ r ↑ V ⤳ t
/// ```
pub fn check(
    context: &Context,
    term: &Rc<RawTerm>,
    expected: &Rc<Type>,
) -> Result<Rc<Term>, TypeError> {
    use syntax::core::{Constant, RawConstant};

    /// ```text
    /// Γ ⊢ r ↑ c ⤳ t
    /// ```
    fn check_const(c: &RawConstant, c_ty: &Constant) -> Option<Constant> {
        match (c, c_ty) {
            // FIXME: overflow?
            (&RawConstant::Int(value), &Constant::U8Type) => Some(Constant::U8(value as u8)),
            (&RawConstant::Int(value), &Constant::U16Type) => Some(Constant::U16(value as u16)),
            (&RawConstant::Int(value), &Constant::U32Type) => Some(Constant::U32(value as u32)),
            (&RawConstant::Int(value), &Constant::U64Type) => Some(Constant::U64(value)),
            (&RawConstant::Int(value), &Constant::I8Type) => Some(Constant::I8(value as i8)),
            (&RawConstant::Int(value), &Constant::I16Type) => Some(Constant::I16(value as i16)),
            (&RawConstant::Int(value), &Constant::I32Type) => Some(Constant::I32(value as i32)),
            (&RawConstant::Int(value), &Constant::I64Type) => Some(Constant::I64(value as i64)),
            (&RawConstant::Int(value), &Constant::F32Type) => Some(Constant::F32(value as f32)),
            (&RawConstant::Int(value), &Constant::F64Type) => Some(Constant::F64(value as f64)),
            (&RawConstant::Float(value), &Constant::F32Type) => Some(Constant::F32(value as f32)),
            (&RawConstant::Float(value), &Constant::F64Type) => Some(Constant::F64(value)),
            (_, _) => None,
        }
    }

    match (&**term, &**expected) {
        // We infer the type of the argument (`τ₁`) of the lambda from the
        // supplied pi type, then 'push' it into the elaborated term, along with
        // the elaborated body (`v`).
        //
        //  1.  Γ, Πx:V₁ ⊢ r ↑ V₂ ⤳ t
        // ────────────────────────────────────── (CHECK/LAM)
        //      Γ ⊢ λx.r ↑ Πx:V₁.V₂ ⤳ λx:V₁.t
        (&RawTerm::Lam(meta, ref lam_scope), &Value::Pi(ref pi_scope)) => {
            let ((lam_name, Embed(lam_ann)), lam_body, (pi_name, Embed(pi_ann)), pi_body) =
                nameless::unbind2(lam_scope.clone(), pi_scope.clone());

            // Elaborate the hole, if it exists
            if let RawTerm::Hole(_) = *lam_ann {
                let body_context = context.extend_pi(pi_name, pi_ann.clone());
                let elab_param = (lam_name, Embed(Rc::new(Term::from(&*pi_ann))));
                let elab_lam_body = check(&body_context, &lam_body, &pi_body)?; // 1.

                return Ok(Term::Lam(meta, Scope::bind(elab_param, elab_lam_body)).into());
            }

            // TODO: We might want to optimise for this case, rather than
            // falling through to `infer` and reunbinding at INFER/LAM
        },
        (&RawTerm::Constant(meta, ref c), &Value::Constant(ref c_ty)) => {
            if let Some(c) = check_const(c, c_ty) {
                return Ok(Term::Constant(meta, c).into());
            }
        },
        (&RawTerm::Lam(_, _), _) => {
            return Err(TypeError::UnexpectedFunction {
                span: term.span(),
                expected: expected.clone(),
            });
        },
        (&RawTerm::Hole(meta), _) => {
            return Err(TypeError::UnableToElaborateHole {
                span: meta.span,
                expected: Some(expected.clone()),
            });
        },
        _ => {},
    }

    // Flip the direction of the type checker, comparing the type of the
    // expected term for [alpha equivalence] with the inferred term.
    //
    //  1.  Γ ⊢ r ↓ V₂ ⤳ t
    //  2.  V₁ ≡ V₂
    // ─────────────────────── (CHECK/INFER)
    //      Γ ⊢ r ↑ V₁ ⤳ t
    //
    // NOTE: We could change 2. to check for subtyping instead of alpha
    // equivalence. This could be useful for implementing a cumulative
    // universe hierarchy.
    //
    // [alpha equivalence]: https://en.wikipedia.org/wiki/Lambda_calculus#Alpha_equivalence

    let (elab_term, inferred_ty) = infer(context, term)?; // 1.

    match Type::term_eq(&inferred_ty, expected) {
        true => Ok(elab_term),
        false => Err(TypeError::Mismatch {
            span: term.span(),
            found: inferred_ty,
            expected: expected.clone(),
        }),
    }
}

/// Type inference of terms
///
/// Under the assumptions in the context, synthesize a type for the given term
/// and return its elaborated form.
///
/// ```text
/// Γ ⊢ r ↓ V ⤳ t
/// ```
pub fn infer(context: &Context, term: &Rc<RawTerm>) -> Result<(Rc<Term>, Rc<Type>), TypeError> {
    use std::cmp;

    use syntax::core::{RawConstant, SourceMeta};

    /// Ensures that the given term is a universe, returning the level of that
    /// universe and its elaborated form.
    ///
    /// ```text
    /// Γ ⊢ R ↓ Typeᵢ ⤳ T
    /// ```
    fn infer_universe(
        context: &Context,
        term: &Rc<RawTerm>,
    ) -> Result<(Rc<Term>, Level), TypeError> {
        let (elab, ty) = infer(context, term)?;
        match *ty {
            Value::Universe(level) => Ok((elab, level)),
            _ => Err(TypeError::ExpectedUniverse {
                span: term.span(),
                found: ty,
            }),
        }
    }

    /// ```text
    /// Γ ⊢ r ↓ c ⤳ t
    /// ```
    fn infer_const(meta: SourceMeta, c: &RawConstant) -> Result<(Rc<Term>, Rc<Type>), TypeError> {
        use syntax::core::{Constant as C, RawConstant as RawC};

        let (term, ty) = match *c {
            RawC::String(ref value) => (C::String(value.clone()), Value::Constant(C::StringType)),
            RawC::Char(value) => (C::Char(value), Value::Constant(C::CharType)),
            RawC::Int(_) => return Err(TypeError::AmbiguousIntLiteral { span: meta.span }),
            RawC::Float(_) => return Err(TypeError::AmbiguousFloatLiteral { span: meta.span }),
            RawC::StringType => (C::StringType, Value::Universe(Level(0))),
            RawC::CharType => (C::CharType, Value::Universe(Level(0))),
            RawC::U8Type => (C::U8Type, Value::Universe(Level(0))),
            RawC::U16Type => (C::U16Type, Value::Universe(Level(0))),
            RawC::U32Type => (C::U32Type, Value::Universe(Level(0))),
            RawC::U64Type => (C::U64Type, Value::Universe(Level(0))),
            RawC::I8Type => (C::I8Type, Value::Universe(Level(0))),
            RawC::I16Type => (C::I16Type, Value::Universe(Level(0))),
            RawC::I32Type => (C::I32Type, Value::Universe(Level(0))),
            RawC::I64Type => (C::I64Type, Value::Universe(Level(0))),
            RawC::F32Type => (C::F32Type, Value::Universe(Level(0))),
            RawC::F64Type => (C::F64Type, Value::Universe(Level(0))),
        };

        Ok((Term::Constant(meta, term).into(), ty.into()))
    }

    match **term {
        //  1.  Γ ⊢ R ↓ Typeᵢ ⤳ T
        //  2.  Γ ⊢ T ⇒ V
        //  3.  Γ ⊢ r ↑ V ⤳ t
        // ───────────────────────────── (INFER/ANN)
        //      Γ ⊢ r:R ↓ V ⤳ t:T
        RawTerm::Ann(meta, ref expr, ref ty) => {
            let (elab_ty, _) = infer_universe(context, ty)?; // 1.
            let simp_ty = normalize(context, &elab_ty)?; // 2.
            let elab_expr = check(context, expr, &simp_ty)?; // 3.
            Ok((Term::Ann(meta, elab_expr, elab_ty).into(), simp_ty))
        },

        // ───────────────────────────────── (INFER/TYPE)
        //  Γ ⊢ Typeᵢ ↓ Typeᵢ₊₁ ⤳ Typeᵢ
        RawTerm::Universe(meta, level) => Ok((
            Term::Universe(meta, level).into(),
            Value::Universe(level.succ()).into(),
        )),

        RawTerm::Hole(meta) => Err(TypeError::UnableToElaborateHole {
            span: meta.span,
            expected: None,
        }),

        RawTerm::Constant(meta, ref c) => infer_const(meta, c),

        RawTerm::Var(meta, ref var) => match *var {
            Var::Free(ref name) => match context.lookup_binder(name) {
                //  1.  λx:V ∈ Γ
                // ─────────────────────── (INFER/VAR-LAM)
                //      Γ ⊢ x ↓ V ⤳ x
                //
                //  2.  Πx:V ∈ Γ
                // ─────────────────────── (INFER/VAR-PI)
                //      Γ ⊢ x ↓ V ⤳ x
                //
                //  3.  let x:V = v ∈ Γ
                // ─────────────────────── (INFER/VAR-LET)
                //      Γ ⊢ x ↓ V ⤳ x
                Some(&Binder::Lam { ann: ref ty, .. })
                | Some(&Binder::Pi { ann: ref ty, .. })
                | Some(&Binder::Let { ann: ref ty, .. }) => {
                    Ok((Term::Var(meta, var.clone()).into(), ty.clone()))
                },

                None => Err(TypeError::UndefinedName {
                    var_span: term.span(),
                    name: name.clone(),
                }),
            },

            // We should always be substituting bound variables with fresh
            // variables when entering scopes using `unbind`, so if we've
            // encountered one here this is definitely a bug!
            Var::Bound(ref name, index) => Err(InternalError::UnsubstitutedDebruijnIndex {
                span: term.span(),
                name: name.clone(),
                index: index,
            }.into()),
        },

        //  1.  Γ ⊢ R₁ ↓ Typeᵢ ⤳ T₁
        //  2.  Γ ⊢ T₁ ⇒ T₁'
        //  3.  Γ, Πx:T₁' ⊢ R₂ ↑ Typeⱼ ⤳ T₂
        //  4.  k = max(i, j)
        // ────────────────────────────────────────── (INFER/PI)
        //      Γ ⊢ Πx:R₁.R₂ ↓ Typeₖ ⤳ Πx:T₁.T₂
        RawTerm::Pi(meta, ref scope) => {
            let ((name, Embed(param_ann)), body) = nameless::unbind(scope.clone());

            let (elab_ann, level_ann) = infer_universe(context, &param_ann)?; // 1.
            let simp_ann = normalize(context, &elab_ann)?; // 2.
            let body_context = context.extend_pi(name.clone(), simp_ann);
            let (elab_body, level_body) = infer_universe(&body_context, &body)?; // 3.

            let elab_param = (name, Embed(elab_ann));
            let elab_pi = Term::Pi(meta, Scope::bind(elab_param, elab_body)).into();
            let level = cmp::max(level_ann, level_body); // 4.

            Ok((elab_pi, Value::Universe(level).into()))
        },

        //  1.  Γ ⊢ R ↓ Typeᵢ ⤳ T
        //  2.  Γ ⊢ T ⇒ V₁
        //  3.  Γ, λx:V₁ ⊢ r ↓ V₂ ⤳ t
        // ───────────────────────────────────────── (INFER/LAM)
        //      Γ ⊢ λx:R.r ↓ Πx:V₁.V₂ ⤳ λx:T.t
        RawTerm::Lam(meta, ref scope) => {
            let ((name, Embed(param_ann)), body) = nameless::unbind(scope.clone());

            // Check for holes before entering to ensure we get a nice error
            if let RawTerm::Hole(_) = *param_ann {
                return Err(TypeError::FunctionParamNeedsAnnotation {
                    param_span: ByteSpan::default(), // TODO: param.span(),
                    var_span: None,
                    name: name.clone(),
                });
            }

            let (lam_ann, _) = infer_universe(context, &param_ann)?; // 1.
            let pi_ann = normalize(context, &lam_ann)?; // 2.
            let body_ctx = context.extend_lam(name.clone(), pi_ann.clone());
            let (lam_body, pi_body) = infer(&body_ctx, &body)?; // 3.

            let lam_param = (name.clone(), Embed(lam_ann));
            let pi_param = (name.clone(), Embed(pi_ann));

            Ok((
                Term::Lam(meta, Scope::bind(lam_param, lam_body)).into(),
                Value::Pi(Scope::bind(pi_param, pi_body)).into(),
            ))
        },

        //  1.  Γ ⊢ r₁ ↓ Πx:V₁.V₂ ⤳ t₁
        //  2.  Γ ⊢ r₂ ↑ V₁ ⤳ t₂
        //  3.  Γ, let x:V₁ = t₂ ⊢ V₂ ⇒ V₂'
        // ────────────────────────────────────── (INFER/APP)
        //      Γ ⊢ r₁ r₂ ↓ V₂' ⤳ t₁ t₂
        RawTerm::App(meta, ref fn_expr, ref arg_expr) => {
            let (elab_fn_expr, fn_ty) = infer(context, fn_expr)?; // 1.

            match *fn_ty {
                Value::Pi(ref scope) => {
                    let ((name, Embed(param_ann)), body) = nameless::unbind(scope.clone());

                    let arg_expr = check(context, arg_expr, &param_ann)?; // 2.

                    // 3.
                    let body = normalize(
                        &context.extend_let(name, param_ann, arg_expr.clone()),
                        &Rc::new(Term::from(&*body)),
                    )?;

                    Ok((Term::App(meta, elab_fn_expr, arg_expr).into(), body))
                },
                _ => Err(TypeError::ArgAppliedToNonFunction {
                    fn_span: fn_expr.span(),
                    arg_span: arg_expr.span(),
                    found: fn_ty.clone(),
                }),
            }
        },
    }
}
