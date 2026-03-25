use super::{
    diagnostics::Diagnostic,
    types::{Type, Value},
    AuthorizationContext,
};

#[derive(Clone, Debug)]
pub struct FunctionInput {
    pub name: String,
    pub documentation: String,
    pub typing: Vec<Type>,
    pub optional: bool,
}

#[derive(Clone, Debug)]
pub struct FunctionOutput {
    pub documentation: String,
    pub typing: Type,
}

#[derive(Clone, Debug)]
pub struct FunctionSpecification {
    pub name: String,
    pub documentation: String,
    pub inputs: Vec<FunctionInput>,
    pub output: FunctionOutput,
    pub example: String,
    pub snippet: String,
    pub runner: FunctionRunner,
    pub checker: FunctionChecker,
}

type FunctionRunner =
    fn(&FunctionSpecification, &AuthorizationContext, &Vec<Value>) -> Result<Value, Diagnostic>;
type FunctionChecker =
    fn(&FunctionSpecification, &AuthorizationContext, &Vec<Type>) -> Result<Type, Diagnostic>;

pub trait FunctionImplementation {
    fn check_instantiability(
        _fn_spec: &FunctionSpecification,
        _auth_ctx: &AuthorizationContext,
        _args: &Vec<Type>,
    ) -> Result<Type, Diagnostic>;

    fn run(
        _fn_spec: &FunctionSpecification,
        _auth_ctx: &AuthorizationContext,
        _args: &Vec<Value>,
    ) -> Result<Value, Diagnostic>;
}

pub fn fn_diag_with_ctx(
    namespace: String,
) -> impl Fn(&FunctionSpecification, String) -> Diagnostic {
    let fn_diag_with_ctx = move |fn_spec: &FunctionSpecification, e: String| -> Diagnostic {
        Diagnostic::error_from_string(format!("function '{}::{}': {}", namespace, fn_spec.name, e))
    };
    return fn_diag_with_ctx;
}

pub fn arg_checker_with_ctx(
    namespace: String,
) -> impl Fn(&FunctionSpecification, &Vec<Value>) -> Result<(), Diagnostic> {
    let fn_checker =
        move |fn_spec: &FunctionSpecification, args: &Vec<Value>| -> Result<(), Diagnostic> {
            for (i, input) in fn_spec.inputs.iter().enumerate() {
                if !input.optional {
                    if let Some(arg) = args.get(i) {
                        let mut has_type_match = false;
                        for typing in input.typing.iter() {
                            let arg_type = arg.get_type();
                            // special case if both are addons: we don't want to be so strict that
                            // we check the addon id here
                            if let Type::Addon(_) = arg_type {
                                if let Type::Addon(_) = typing {
                                    has_type_match = true;
                                    break;
                                }
                            }
                            // special case for empty arrays
                            if let Type::Array(_) = arg_type {
                                if arg.expect_array().len() == 0 {
                                    has_type_match = true;
                                    break;
                                }
                            }
                            // we don't have an "any" type, so if the array is of type null, we won't check types
                            if let Type::Array(inner) = typing {
                                if matches!(**inner, Type::Null | Type::TypedNull(_)) {
                                    has_type_match = true;
                                    break;
                                }
                            }
                            if arg_type.eq(typing) {
                                has_type_match = true;
                                break;
                            }
                        }
                        if !has_type_match {
                            let expected_types = input
                                .typing
                                .iter()
                                .map(|t| t.to_string())
                                .collect::<Vec<String>>()
                                .join(",");
                            return Err(Diagnostic::error_from_string(format!(
                            "function '{}::{}' argument #{} ({}) should be of type ({}), found {}",
                            namespace,
                            fn_spec.name,
                            i + 1,
                            input.name,
                            expected_types,
                            arg.get_type().to_string()
                        )));
                        }
                    } else {
                        return Err(Diagnostic::error_from_string(format!(
                            "function '{}::{}' missing required argument #{} ({})",
                            namespace,
                            fn_spec.name,
                            i + 1,
                            input.name,
                        )));
                    }
                }
            }
            Ok(())
        };
    return fn_checker;
}
