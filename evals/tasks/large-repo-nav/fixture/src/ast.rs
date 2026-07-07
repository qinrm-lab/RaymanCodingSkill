/// An operator applied to two sub-expressions.
#[derive(Debug, Clone, Copy)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

/// A parsed arithmetic expression tree.
#[derive(Debug)]
pub enum Expr {
    Num(i64),
    Binary {
        op: Op,
        left: Box<Expr>,
        right: Box<Expr>,
    },
}
