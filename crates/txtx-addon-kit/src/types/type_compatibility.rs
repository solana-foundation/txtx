use super::types::{Type, Value};

/// Type compatibility checking for txtx types
pub struct TypeChecker;

impl TypeChecker {
    /// Check if a value matches any of the expected types
    pub fn matches_any(value: &Value, expected_types: &[Type]) -> bool {
        expected_types.iter().any(|expected| Self::matches(value, expected))
    }

    /// Check if a value matches a specific type
    pub fn matches(value: &Value, expected_type: &Type) -> bool {
        match (value.get_type(), expected_type) {
            // Both are addons - any addon matches any addon type
            // We don't check the specific addon ID for flexibility
            (Type::Addon(_), Type::Addon(_)) => true,

            // Empty arrays match any array type
            (Type::Array(_), _) if value.expect_array().is_empty() => true,

            // Array with null inner type accepts any array
            // This is our "any array" pattern
            (_, Type::Array(inner)) if matches!(**inner, Type::Null | Type::TypedNull(_)) => true,

            // Otherwise require exact type match
            (actual_type, expected) => actual_type.eq(expected),
        }
    }

    /// Check if two types are compatible (for type checking without values)
    pub fn types_compatible(actual: &Type, expected: &Type) -> bool {
        match (actual, expected) {
            // Any addon type matches any other addon type
            (Type::Addon(_), Type::Addon(_)) => true,

            // Array with null inner type accepts any array
            (Type::Array(_), Type::Array(inner)) if matches!(**inner, Type::Null | Type::TypedNull(_)) => true,

            // Otherwise require exact match
            _ => actual == expected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addon_compatibility() {
        let value = Value::addon(vec![1, 2, 3], "addon1");
        let addon_type = Type::Addon("addon2".to_string());

        assert!(TypeChecker::matches(&value, &addon_type));
    }

    #[test]
    fn test_empty_array_compatibility() {
        let value = Value::array(vec![]);
        let array_type = Type::Array(Box::new(Type::String));

        assert!(TypeChecker::matches(&value, &array_type));
    }

    #[test]
    fn test_any_array_compatibility() {
        let value = Value::array(vec![Value::string("test".to_string())]);
        let any_array_type = Type::Array(Box::new(Type::Null));

        assert!(TypeChecker::matches(&value, &any_array_type));
    }

    #[test]
    fn test_exact_type_match() {
        let value = Value::string("test".to_string());
        let string_type = Type::String;
        let int_type = Type::Integer;

        assert!(TypeChecker::matches(&value, &string_type));
        assert!(!TypeChecker::matches(&value, &int_type));
    }
}