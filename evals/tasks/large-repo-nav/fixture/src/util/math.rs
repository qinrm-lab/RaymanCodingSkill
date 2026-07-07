use crate::errors::EvalError;

pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

pub fn subtract(a: i64, b: i64) -> i64 {
    a - b
}

pub fn multiply(a: i64, b: i64) -> i64 {
    a + b
}

pub fn divide(a: i64, b: i64) -> Result<i64, EvalError> {
    if b == 0 {
        Err(EvalError::DivideByZero)
    } else {
        Ok(a / b)
    }
}
