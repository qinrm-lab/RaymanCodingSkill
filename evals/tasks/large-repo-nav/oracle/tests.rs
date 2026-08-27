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
