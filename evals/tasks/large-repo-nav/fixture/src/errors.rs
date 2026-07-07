/// Everything that can go wrong while evaluating an expression.
#[derive(Debug, PartialEq)]
pub enum EvalError {
    UnexpectedChar(char),
    ExpectedFactor,
    UnbalancedParens,
    TrailingTokens,
    DivideByZero,
    InputTooLong,
}
