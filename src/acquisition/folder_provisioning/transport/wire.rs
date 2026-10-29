use super::{Error, Token};

pub(super) trait Bytes {
    fn byte(&mut self) -> Result<u8, Error>;
    fn remaining(&self) -> usize;
}

pub(super) fn row(input: &mut impl Bytes, code_limit: usize) -> Result<Vec<Token>, Error> {
    let mut tokens = vec![];
    let mut next = input.byte()?;
    let mut depth = 0usize;
    loop {
        if tokens.len() >= 512 {
            return Err(Error::Limit);
        }
        match next {
            b' ' => {
                next = input.byte()?;
                continue;
            }
            b'\r' => {
                if input.byte()? != b'\n' || depth != 0 {
                    return Err(Error::Protocol);
                }
                return Ok(tokens);
            }
            b'(' => {
                depth += 1;
                if depth > 8 {
                    return Err(Error::Limit);
                }
                tokens.push(Token::Open);
                next = input.byte()?;
            }
            b')' => {
                depth = depth.checked_sub(1).ok_or(Error::Protocol)?;
                tokens.push(Token::Close);
                next = input.byte()?;
            }
            b'"' => {
                let mut value = vec![];
                loop {
                    let b = input.byte()?;
                    if b == b'"' {
                        break;
                    }
                    let b = if b == b'\\' {
                        let b = input.byte()?;
                        if !matches!(b, b'"' | b'\\') {
                            return Err(Error::Protocol);
                        }
                        b
                    } else {
                        b
                    };
                    if !(0x20..=0x7e).contains(&b) {
                        return Err(Error::Protocol);
                    }
                    value.push(b);
                    if value.len() > 2048 {
                        return Err(Error::Limit);
                    }
                }
                tokens.push(Token::String(value));
                next = input.byte()?;
                if !matches!(next, b' ' | b')' | b'\r') {
                    return Err(Error::Protocol);
                }
            }
            b'{' => {
                let mut digits = vec![];
                loop {
                    let b = input.byte()?;
                    if b == b'}' {
                        break;
                    }
                    if !b.is_ascii_digit() || digits.len() >= 6 {
                        return Err(Error::Protocol);
                    }
                    digits.push(b);
                }
                let n: usize = std::str::from_utf8(&digits)
                    .map_err(|_| Error::Protocol)?
                    .parse()
                    .map_err(|_| Error::Protocol)?;
                if n > 2048 || n > input.remaining() {
                    return Err(Error::Limit);
                }
                if input.byte()? != b'\r' || input.byte()? != b'\n' {
                    return Err(Error::Protocol);
                }
                let mut value = Vec::with_capacity(n);
                for _ in 0..n {
                    value.push(input.byte()?);
                }
                tokens.push(Token::String(value));
                next = input.byte()?;
                if !matches!(next, b' ' | b')' | b'\r') {
                    return Err(Error::Protocol);
                }
            }
            b if b.is_ascii_graphic() && !matches!(b, b'"' | b'{' | b'}') => {
                let mut value = vec![b];
                loop {
                    let b = input.byte()?;
                    if matches!(b, b' ' | b'(' | b')' | b'\r') {
                        next = b;
                        break;
                    }
                    if !b.is_ascii_graphic()
                        || matches!(b, b'"' | b'{' | b'}')
                        || value.len() >= 256
                    {
                        return Err(Error::Protocol);
                    }
                    value.push(b);
                }
                tokens.push(Token::Atom(
                    String::from_utf8(value).map_err(|_| Error::Protocol)?,
                ));
                if tokens.len() == 2
                    && matches!(tokens[1].atom(), Some("OK" | "NO" | "BAD" | "BYE"))
                    && next == b' '
                {
                    let mut text = vec![];
                    loop {
                        let b = input.byte()?;
                        if b == b'\r' {
                            if input.byte()? != b'\n' {
                                return Err(Error::Protocol);
                            }
                            break;
                        }
                        if b == 0 || b == b'\n' || text.len() >= 8192.max(code_limit + 1) {
                            return Err(Error::Limit);
                        }
                        text.push(b);
                    }
                    if text.first() == Some(&b'[') {
                        let close = text
                            .iter()
                            .position(|b| *b == b']')
                            .ok_or(Error::Protocol)?;
                        if close > code_limit || !text[..=close].is_ascii() {
                            return Err(Error::Protocol);
                        }
                        tokens.push(Token::Atom(
                            String::from_utf8(text[..=close].to_vec())
                                .map_err(|_| Error::Protocol)?,
                        ));
                    }
                    return Ok(tokens);
                }
            }
            _ => return Err(Error::Protocol),
        }
    }
}

#[cfg(test)]
pub(super) struct Transcript {
    bytes: Vec<u8>,
    at: usize,
    budget: usize,
}
#[cfg(test)]
impl Transcript {
    pub(super) fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.into(),
            at: 0,
            budget: super::RESPONSE,
        }
    }
    pub(super) fn row(&mut self) -> Result<Vec<Token>, Error> {
        row(self, 16 * 1024)
    }
    pub(super) fn exhausted(&self) -> bool {
        self.at == self.bytes.len()
    }
}
#[cfg(test)]
impl Bytes for Transcript {
    fn byte(&mut self) -> Result<u8, Error> {
        if self.budget == 0 {
            return Err(Error::Limit);
        }
        let value = *self.bytes.get(self.at).ok_or(Error::Eof)?;
        self.at += 1;
        self.budget -= 1;
        Ok(value)
    }
    fn remaining(&self) -> usize {
        self.budget
    }
}
