use crate::errors::EvalError;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Num(i64),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

/// Turn source text into a flat token stream.
pub fn tokenize(input: &str) -> Result<Vec<Token>, EvalError> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            '+' => {
                tokens.push(Token::Plus);
                chars.next();
            }
            '-' => {
                tokens.push(Token::Minus);
                chars.next();
            }
            '*' => {
                tokens.push(Token::Star);
                chars.next();
            }
            '/' => {
                tokens.push(Token::Slash);
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            '0'..='9' => {
                let mut n = 0i64;
                while let Some(&d) = chars.peek() {
                    match d.to_digit(10) {
                        Some(digit) => {
                            n = n * 10 + digit as i64;
                            chars.next();
                        }
                        None => break,
                    }
                }
                tokens.push(Token::Num(n));
            }
            other => return Err(EvalError::UnexpectedChar(other)),
        }
    }
    Ok(tokens)
}
