use crate::ast::{Expr, Op};
use crate::errors::EvalError;
use crate::tokenizer::Token;

/// Parse a token stream into an expression tree, honouring `* /` over `+ -`.
pub fn parse(tokens: &[Token]) -> Result<Expr, EvalError> {
    let mut p = Parser { tokens, pos: 0 };
    let expr = p.expression()?;
    if p.pos != tokens.len() {
        return Err(EvalError::TrailingTokens);
    }
    Ok(expr)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn expression(&mut self) -> Result<Expr, EvalError> {
        let mut left = self.term()?;
        while let Some(op) = match self.peek() {
            Some(Token::Plus) => Some(Op::Add),
            Some(Token::Minus) => Some(Op::Sub),
            _ => None,
        } {
            self.pos += 1;
            let right = self.term()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn term(&mut self) -> Result<Expr, EvalError> {
        let mut left = self.factor()?;
        while let Some(op) = match self.peek() {
            Some(Token::Star) => Some(Op::Mul),
            Some(Token::Slash) => Some(Op::Div),
            _ => None,
        } {
            self.pos += 1;
            let right = self.factor()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn factor(&mut self) -> Result<Expr, EvalError> {
        match self.peek() {
            Some(Token::Num(n)) => {
                let n = *n;
                self.pos += 1;
                Ok(Expr::Num(n))
            }
            Some(Token::LParen) => {
                self.pos += 1;
                let inner = self.expression()?;
                match self.peek() {
                    Some(Token::RParen) => {
                        self.pos += 1;
                        Ok(inner)
                    }
                    _ => Err(EvalError::UnbalancedParens),
                }
            }
            _ => Err(EvalError::ExpectedFactor),
        }
    }
}
