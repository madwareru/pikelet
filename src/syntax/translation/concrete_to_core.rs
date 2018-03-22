use codespan::ByteSpan;
use nameless::{Embed, GenId, Scope, Var};

use syntax::concrete;
use syntax::core;

/// Translate something to the corresponding core representation
pub trait ToCore<T> {
    fn to_core(&self) -> T;
}

/// Convert a sugary pi type from something like:
///
/// ```text
/// (a b : t1) -> t3
/// ```
///
/// To a bunch of nested pi types like:
///
/// ```text
/// (a : t1) -> (b : t1) -> t3
/// ```
fn pi_to_core(
    param_names: &[(ByteSpan, String)],
    ann: &concrete::Term,
    body: &concrete::Term,
) -> core::RcRawTerm {
    let ann = ann.to_core();
    let mut term = body.to_core();

    for &(span, ref name) in param_names.iter().rev() {
        // This could be wrong... :/
        term = core::RawTerm::Pi(
            core::SourceMeta {
                span: span.to(term.span()),
            },
            Scope::bind((core::Name::user(name.clone()), Embed(ann.clone())), term),
        ).into();
    }

    term
}

/// Convert a sugary lambda from something like:
///
/// ```text
/// \(a b : t1) c (d : t2) => t3
/// ```
///
/// To a bunch of nested lambdas like:
///
/// ```text
/// \(a : t1) => \(b : t1) => \c => \(d : t2) => t3
/// ```
fn lam_to_core(
    params: &[(Vec<(ByteSpan, String)>, Option<Box<concrete::Term>>)],
    body: &concrete::Term,
) -> core::RcRawTerm {
    let mut term = body.to_core();

    for &(ref names, ref ann) in params.iter().rev() {
        for &(span, ref name) in names.iter().rev() {
            let name = core::Name::user(name.clone());
            let meta = core::SourceMeta {
                span: span.to(term.span()),
            };
            let ann = match *ann {
                None => core::RawTerm::Hole(core::SourceMeta::default()).into(),
                Some(ref ann) => ann.to_core(),
            };
            term = core::RawTerm::Lam(meta, Scope::bind((name, Embed(ann)), term)).into();
        }
    }

    term
}

impl ToCore<core::RawModule> for concrete::Module {
    /// Convert the module in the concrete syntax to a module in the core syntax
    fn to_core(&self) -> core::RawModule {
        match *self {
            concrete::Module::Valid {
                ref name,
                ref declarations,
            } => {
                // The type claims that we have encountered so far! We'll use these when
                // we encounter their corresponding definitions later as type annotations
                let mut prev_claim = None;
                // The definitions, desugared from the concrete syntax
                let mut definitions = Vec::<core::RawDefinition>::new();

                for declaration in declarations {
                    match *declaration {
                        concrete::Declaration::Import { .. } => {
                            unimplemented!("import declarations")
                        },
                        concrete::Declaration::Claim {
                            name: (_, ref name),
                            ref ann,
                            ..
                        } => match prev_claim.take() {
                            Some((name, ann)) => {
                                let term = core::RawTerm::Hole(core::SourceMeta::default()).into();
                                definitions.push(core::RawDefinition { name, term, ann });
                            },
                            None => prev_claim = Some((name.clone(), ann.to_core())),
                        },
                        concrete::Declaration::Definition {
                            name: (_, ref name),
                            ref params,
                            ref body,
                            ..
                        } => {
                            let default_meta = core::SourceMeta::default();

                            match prev_claim.take() {
                                None => definitions.push(core::RawDefinition {
                                    name: name.clone(),
                                    ann: core::RawTerm::Hole(default_meta).into(),
                                    term: lam_to_core(params, body),
                                }),
                                Some((claim_name, ann)) => {
                                    if claim_name == *name {
                                        definitions.push(core::RawDefinition {
                                            name: name.clone(),
                                            ann,
                                            term: lam_to_core(params, body),
                                        });
                                    } else {
                                        definitions.push(core::RawDefinition {
                                            name: claim_name.clone(),
                                            ann,
                                            term: core::RawTerm::Hole(default_meta).into(),
                                        });
                                        definitions.push(core::RawDefinition {
                                            name: name.clone(),
                                            ann: core::RawTerm::Hole(default_meta).into(),
                                            term: lam_to_core(params, body),
                                        });
                                    }
                                },
                            };
                        },
                        concrete::Declaration::Error(_) => unimplemented!("error recovery"),
                    }
                }

                core::RawModule {
                    name: name.1.clone(),
                    definitions,
                }
            },
            concrete::Module::Error(_) => unimplemented!("error recovery"),
        }
    }
}

impl ToCore<core::RcRawTerm> for concrete::Term {
    /// Convert a term in the concrete syntax into a core term
    fn to_core(&self) -> core::RcRawTerm {
        let meta = core::SourceMeta { span: self.span() };
        match *self {
            concrete::Term::Parens(_, ref term) => term.to_core(),
            concrete::Term::Ann(ref expr, ref ty) => {
                let expr = expr.to_core().into();
                let ty = ty.to_core().into();

                core::RawTerm::Ann(meta, expr, ty).into()
            },
            concrete::Term::Universe(_, level) => {
                core::RawTerm::Universe(meta, core::Level(level.unwrap_or(0))).into()
            },
            concrete::Term::Hole(_) => core::RawTerm::Hole(meta).into(),
            concrete::Term::String(_, ref value) => {
                core::RawTerm::Constant(meta, core::RawConstant::String(value.clone())).into()
            },
            concrete::Term::Char(_, value) => {
                core::RawTerm::Constant(meta, core::RawConstant::Char(value)).into()
            },
            concrete::Term::Int(_, value) => {
                core::RawTerm::Constant(meta, core::RawConstant::Int(value)).into()
            },
            concrete::Term::Float(_, value) => {
                core::RawTerm::Constant(meta, core::RawConstant::Float(value)).into()
            },
            concrete::Term::Var(_, ref x) => {
                core::RawTerm::Var(meta, Var::Free(core::Name::user(x.clone()))).into()
            },
            concrete::Term::Pi(_, (ref names, ref ann), ref body) => pi_to_core(names, ann, body),
            concrete::Term::Lam(_, ref params, ref body) => lam_to_core(params, body),
            concrete::Term::Arrow(ref ann, ref body) => {
                let name = core::Name::from(GenId::fresh());
                let ann = ann.to_core();
                let body = body.to_core();

                core::RawTerm::Pi(meta, Scope::bind((name, Embed(ann)), body)).into()
            },
            concrete::Term::App(ref fn_expr, ref arg) => {
                let fn_expr = fn_expr.to_core();
                let arg = arg.to_core();

                core::RawTerm::App(meta, fn_expr, arg).into()
            },
            concrete::Term::Error(_) => unimplemented!("error recovery"),
        }
    }
}

#[cfg(test)]
mod to_core {
    use codespan::{CodeMap, FileName};

    use library;
    use syntax::parse;

    use super::*;

    fn parse(src: &str) -> core::RcRawTerm {
        let mut codemap = CodeMap::new();
        let filemap = codemap.add_filemap(FileName::virtual_("test"), src.into());

        let (concrete_term, errors) = parse::term(&filemap);
        assert!(errors.is_empty());

        concrete_term.to_core()
    }

    mod module {
        use super::*;

        #[test]
        fn parse_prelude() {
            let mut codemap = CodeMap::new();
            let filemap = codemap.add_filemap(FileName::virtual_("test"), library::PRELUDE.into());

            let (concrete_module, errors) = parse::module(&filemap);
            assert!(errors.is_empty());

            concrete_module.to_core();
        }
    }

    mod term {
        use super::*;

        use syntax::core::{Level, Name, RawTerm, SourceMeta};

        #[test]
        fn var() {
            assert_term_eq!(
                parse(r"x"),
                RawTerm::Var(SourceMeta::default(), Var::Free(Name::user("x"))).into()
            );
        }

        #[test]
        fn var_kebab_case() {
            assert_term_eq!(
                parse(r"or-elim"),
                RawTerm::Var(SourceMeta::default(), Var::Free(Name::user("or-elim"))).into(),
            );
        }

        #[test]
        fn ty() {
            assert_term_eq!(
                parse(r"Type"),
                RawTerm::Universe(SourceMeta::default(), Level(0)).into()
            );
        }

        #[test]
        fn ty_level() {
            assert_term_eq!(
                parse(r"Type 2"),
                RawTerm::Universe(SourceMeta::default(), Level(0).succ().succ()).into()
            );
        }

        #[test]
        fn ann() {
            assert_term_eq!(
                parse(r"Type : Type"),
                RawTerm::Ann(
                    SourceMeta::default(),
                    RawTerm::Universe(SourceMeta::default(), Level(0)).into(),
                    RawTerm::Universe(SourceMeta::default(), Level(0)).into()
                ).into(),
            );
        }

        #[test]
        fn ann_ann_left() {
            assert_term_eq!(
                parse(r"Type : Type : Type"),
                RawTerm::Ann(
                    SourceMeta::default(),
                    RawTerm::Universe(SourceMeta::default(), Level(0)).into(),
                    RawTerm::Ann(
                        SourceMeta::default(),
                        RawTerm::Universe(SourceMeta::default(), Level(0)).into(),
                        RawTerm::Universe(SourceMeta::default(), Level(0)).into()
                    ).into(),
                ).into(),
            );
        }

        #[test]
        fn ann_ann_right() {
            assert_term_eq!(
                parse(r"Type : (Type : Type)"),
                RawTerm::Ann(
                    SourceMeta::default(),
                    RawTerm::Universe(SourceMeta::default(), Level(0)).into(),
                    RawTerm::Ann(
                        SourceMeta::default(),
                        RawTerm::Universe(SourceMeta::default(), Level(0)).into(),
                        RawTerm::Universe(SourceMeta::default(), Level(0)).into()
                    ).into(),
                ).into(),
            );
        }

        #[test]
        fn ann_ann_ann() {
            assert_term_eq!(
                parse(r"(Type : Type) : (Type : Type)"),
                RawTerm::Ann(
                    SourceMeta::default(),
                    RawTerm::Ann(
                        SourceMeta::default(),
                        RawTerm::Universe(SourceMeta::default(), Level(0)).into(),
                        RawTerm::Universe(SourceMeta::default(), Level(0)).into()
                    ).into(),
                    RawTerm::Ann(
                        SourceMeta::default(),
                        RawTerm::Universe(SourceMeta::default(), Level(0)).into(),
                        RawTerm::Universe(SourceMeta::default(), Level(0)).into()
                    ).into(),
                ).into(),
            );
        }

        #[test]
        fn lam_ann() {
            let x = Name::user("x");

            assert_term_eq!(
                parse(r"\x : Type -> Type => x"),
                RawTerm::Lam(
                    SourceMeta::default(),
                    Scope::bind(
                        (
                            x.clone(),
                            Embed(
                                RawTerm::Pi(
                                    SourceMeta::default(),
                                    Scope::bind(
                                        (
                                            Name::user("_"),
                                            Embed(
                                                RawTerm::Universe(SourceMeta::default(), Level(0))
                                                    .into()
                                            )
                                        ),
                                        RawTerm::Universe(SourceMeta::default(), Level(0)).into(),
                                    )
                                ).into()
                            )
                        ),
                        RawTerm::Var(SourceMeta::default(), Var::Free(x)).into(),
                    )
                ).into(),
            );
        }

        #[test]
        fn lam() {
            let x = Name::user("x");
            let y = Name::user("y");

            assert_term_eq!(
                parse(r"\x : (\y => y) => x"),
                RawTerm::Lam(
                    SourceMeta::default(),
                    Scope::bind(
                        (
                            x.clone(),
                            Embed(
                                RawTerm::Lam(
                                    SourceMeta::default(),
                                    Scope::bind(
                                        (
                                            y.clone(),
                                            Embed(RawTerm::Hole(SourceMeta::default()).into())
                                        ),
                                        RawTerm::Var(SourceMeta::default(), Var::Free(y)).into(),
                                    )
                                ).into()
                            ),
                        ),
                        RawTerm::Var(SourceMeta::default(), Var::Free(x)).into(),
                    )
                ).into(),
            );
        }

        #[test]
        fn lam_lam_ann() {
            let x = Name::user("x");
            let y = Name::user("y");

            assert_term_eq!(
                parse(r"\(x y : Type) => x"),
                RawTerm::Lam(
                    SourceMeta::default(),
                    Scope::bind(
                        (
                            x.clone(),
                            Embed(RawTerm::Universe(SourceMeta::default(), Level(0)).into())
                        ),
                        RawTerm::Lam(
                            SourceMeta::default(),
                            Scope::bind(
                                (
                                    y,
                                    Embed(
                                        RawTerm::Universe(SourceMeta::default(), Level(0)).into()
                                    )
                                ),
                                RawTerm::Var(SourceMeta::default(), Var::Free(x)).into(),
                            )
                        ).into(),
                    )
                ).into(),
            );
        }

        #[test]
        fn arrow() {
            assert_term_eq!(
                parse(r"Type -> Type"),
                RawTerm::Pi(
                    SourceMeta::default(),
                    Scope::bind(
                        (
                            Name::user("_"),
                            Embed(RawTerm::Universe(SourceMeta::default(), Level(0)).into())
                        ),
                        RawTerm::Universe(SourceMeta::default(), Level(0)).into(),
                    )
                ).into(),
            );
        }

        #[test]
        fn pi() {
            let x = Name::user("x");

            assert_term_eq!(
                parse(r"(x : Type -> Type) -> x"),
                RawTerm::Pi(
                    SourceMeta::default(),
                    Scope::bind(
                        (
                            x.clone(),
                            Embed(
                                RawTerm::Pi(
                                    SourceMeta::default(),
                                    Scope::bind(
                                        (
                                            Name::user("_"),
                                            Embed(
                                                RawTerm::Universe(SourceMeta::default(), Level(0))
                                                    .into()
                                            )
                                        ),
                                        RawTerm::Universe(SourceMeta::default(), Level(0)).into(),
                                    )
                                ).into()
                            ),
                        ),
                        RawTerm::Var(SourceMeta::default(), Var::Free(x)).into(),
                    )
                ).into(),
            );
        }

        #[test]
        fn pi_pi() {
            let x = Name::user("x");
            let y = Name::user("y");

            assert_term_eq!(
                parse(r"(x y : Type) -> x"),
                RawTerm::Pi(
                    SourceMeta::default(),
                    Scope::bind(
                        (
                            x.clone(),
                            Embed(RawTerm::Universe(SourceMeta::default(), Level(0)).into())
                        ),
                        RawTerm::Pi(
                            SourceMeta::default(),
                            Scope::bind(
                                (
                                    y,
                                    Embed(
                                        RawTerm::Universe(SourceMeta::default(), Level(0)).into()
                                    )
                                ),
                                RawTerm::Var(SourceMeta::default(), Var::Free(x)).into(),
                            )
                        ).into(),
                    )
                ).into(),
            );
        }

        #[test]
        fn pi_arrow() {
            let x = Name::user("x");

            assert_term_eq!(
                parse(r"(x : Type) -> x -> x"),
                RawTerm::Pi(
                    SourceMeta::default(),
                    Scope::bind(
                        (
                            x.clone(),
                            Embed(RawTerm::Universe(SourceMeta::default(), Level(0)).into())
                        ),
                        RawTerm::Pi(
                            SourceMeta::default(),
                            Scope::bind(
                                (
                                    Name::user("_"),
                                    Embed(
                                        RawTerm::Var(SourceMeta::default(), Var::Free(x.clone()))
                                            .into()
                                    )
                                ),
                                RawTerm::Var(SourceMeta::default(), Var::Free(x)).into(),
                            )
                        ).into(),
                    )
                ).into(),
            );
        }

        #[test]
        fn lam_app() {
            let x = Name::user("x");
            let y = Name::user("y");

            assert_term_eq!(
                parse(r"\(x : Type -> Type) (y : Type) => x y"),
                RawTerm::Lam(
                    SourceMeta::default(),
                    Scope::bind(
                        (
                            x.clone(),
                            Embed(
                                RawTerm::Pi(
                                    SourceMeta::default(),
                                    Scope::bind(
                                        (
                                            Name::user("_"),
                                            Embed(
                                                RawTerm::Universe(SourceMeta::default(), Level(0))
                                                    .into()
                                            )
                                        ),
                                        RawTerm::Universe(SourceMeta::default(), Level(0)).into(),
                                    )
                                ).into()
                            ),
                        ),
                        RawTerm::Lam(
                            SourceMeta::default(),
                            Scope::bind(
                                (
                                    y.clone(),
                                    Embed(
                                        RawTerm::Universe(SourceMeta::default(), Level(0)).into()
                                    )
                                ),
                                RawTerm::App(
                                    SourceMeta::default(),
                                    RawTerm::Var(SourceMeta::default(), Var::Free(x)).into(),
                                    RawTerm::Var(SourceMeta::default(), Var::Free(y)).into(),
                                ).into(),
                            )
                        ).into(),
                    )
                ).into(),
            );
        }

        #[test]
        fn id() {
            let x = Name::user("x");
            let a = Name::user("a");

            assert_term_eq!(
                parse(r"\(a : Type) (x : a) => x"),
                RawTerm::Lam(
                    SourceMeta::default(),
                    Scope::bind(
                        (
                            a.clone(),
                            Embed(RawTerm::Universe(SourceMeta::default(), Level(0)).into())
                        ),
                        RawTerm::Lam(
                            SourceMeta::default(),
                            Scope::bind(
                                (
                                    x.clone(),
                                    Embed(RawTerm::Var(SourceMeta::default(), Var::Free(a)).into())
                                ),
                                RawTerm::Var(SourceMeta::default(), Var::Free(x)).into(),
                            )
                        ).into(),
                    )
                ).into(),
            );
        }

        #[test]
        fn id_ty() {
            let a = Name::user("a");

            assert_term_eq!(
                parse(r"(a : Type) -> a -> a"),
                RawTerm::Pi(
                    SourceMeta::default(),
                    Scope::bind(
                        (
                            a.clone(),
                            Embed(RawTerm::Universe(SourceMeta::default(), Level(0)).into())
                        ),
                        RawTerm::Pi(
                            SourceMeta::default(),
                            Scope::bind(
                                (
                                    Name::user("_"),
                                    Embed(
                                        RawTerm::Var(SourceMeta::default(), Var::Free(a.clone()))
                                            .into()
                                    )
                                ),
                                RawTerm::Var(SourceMeta::default(), Var::Free(a)).into(),
                            )
                        ).into(),
                    )
                ).into(),
            );
        }

        mod sugar {
            use super::*;

            #[test]
            fn lam_args() {
                assert_term_eq!(
                    parse(r"\x (y : Type) z => x"),
                    parse(r"\x => \y : Type => \z => x"),
                );
            }

            #[test]
            fn lam_args_multi() {
                assert_term_eq!(
                    parse(r"\(x : Type) (y : Type) z => x"),
                    parse(r"\(x y : Type) z => x"),
                );
            }

            #[test]
            fn pi_args() {
                assert_term_eq!(
                    parse(r"(a : Type) -> (x y z : a) -> x"),
                    parse(r"(a : Type) -> (x : a) -> (y : a) -> (z : a) -> x"),
                );
            }

            #[test]
            fn arrow() {
                assert_term_eq!(
                    parse(r"(a : Type) -> a -> a"),
                    parse(r"(a : Type) -> (x : a) -> a"),
                )
            }
        }
    }
}
