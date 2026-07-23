use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) struct SessionId([u8; 16]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InvalidSessionId;

impl SessionId {
    pub(crate) fn parse_bytes(encoded: &[u8]) -> Result<Self, InvalidSessionId> {
        if encoded.len() != 32 {
            return Err(InvalidSessionId);
        }

        let mut bytes = [0_u8; 16];
        for (index, pair) in encoded.chunks_exact(2).enumerate() {
            let high = decode_nibble(pair[0]).ok_or(InvalidSessionId)?;
            let low = decode_nibble(pair[1]).ok_or(InvalidSessionId)?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SessionId")
            .field(&self.to_string())
            .finish()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = [0_u8; 32];
        for (index, byte) in self.0.iter().copied().enumerate() {
            encoded[index * 2] = HEX[usize::from(byte >> 4)];
            encoded[index * 2 + 1] = HEX[usize::from(byte & 0x0f)];
        }
        // Every byte is selected from the ASCII table above.
        let text = std::str::from_utf8(&encoded).map_err(|_| fmt::Error)?;
        formatter.write_str(text)
    }
}

impl FromStr for SessionId {
    type Err = InvalidSessionId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_bytes(value.as_bytes())
    }
}

fn decode_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::SessionId;

    #[test]
    fn parses_and_displays_canonical_lowercase_hex() {
        let encoded = "00112233445566778899aabbccddeeff";
        let session = encoded.parse::<SessionId>();

        assert!(session.is_ok());
        assert_eq!(
            session.map(|value| value.to_string()),
            Ok(encoded.to_owned())
        );
    }

    #[test]
    fn rejects_wrong_length_uppercase_and_non_hex() {
        assert!(SessionId::parse_bytes(b"").is_err());
        assert!(SessionId::parse_bytes(b"00112233445566778899aabbccddeef").is_err());
        assert!(SessionId::parse_bytes(b"00112233445566778899aabbccddeeff00").is_err());
        assert!(SessionId::parse_bytes(b"00112233445566778899AABBCCDDEEFF").is_err());
        assert!(SessionId::parse_bytes(b"00112233445566778899aabbccddeefg").is_err());
    }
}
