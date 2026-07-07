mod ast;
mod constants;
mod errors;
mod evaluator;
mod parser;
mod tokenizer;
mod util;

pub use errors::EvalError;

/// Evaluate an integer arithmetic expression such as "1 + 2 * 3".
pub fn eval(input: &str) -> Result<i64, EvalError> {
    if input.len() > constants::MAX_INPUT_LEN {
        return Err(EvalError::InputTooLong);
    }
    let normalized = util::strings::normalize_spaces(input);
    let tokens = tokenizer::tokenize(&normalized)?;
    let ast = parser::parse(&tokens)?;
    evaluator::evaluate(&ast)
}

#[cfg(test)]
mod tests {
    use super::{eval, EvalError};

    #[test]
    fn evaluates_with_precedence() {
        assert_eq!(eval("2 * 3").unwrap(), 6);
        assert_eq!(eval("1 + 2 * 3").unwrap(), 7);
        assert_eq!(eval("(1 + 2) * 3").unwrap(), 9);
        assert_eq!(eval("10 - 4 - 3").unwrap(), 3);
    }

    #[test]
    fn division_and_errors() {
        assert_eq!(eval("20 / 4 / 5").unwrap(), 1);
        assert_eq!(eval("1 / 0"), Err(EvalError::DivideByZero));
    }
}
