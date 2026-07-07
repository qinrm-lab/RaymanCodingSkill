use crate::ast::{Expr, Op};
use crate::errors::EvalError;
use crate::util::math;

/// Fold an expression tree down to a single integer.
pub fn evaluate(expr: &Expr) -> Result<i64, EvalError> {
    match expr {
        Expr::Num(n) => Ok(*n),
        Expr::Binary { op, left, right } => {
            let l = evaluate(left)?;
            let r = evaluate(right)?;
            match op {
                Op::Add => Ok(math::add(l, r)),
                Op::Sub => Ok(math::subtract(l, r)),
                Op::Mul => Ok(math::multiply(l, r)),
                Op::Div => math::divide(l, r),
            }
        }
    }
}
