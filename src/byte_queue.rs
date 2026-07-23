use std::collections::VecDeque;
use std::io::{self, Write};

pub(crate) struct ByteQueue {
    bytes: VecDeque<u8>,
    limit: usize,
}

impl ByteQueue {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            limit,
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return false;
        }
        self.bytes.extend(bytes.iter().copied());
        true
    }

    pub(crate) fn flush(&mut self, output: &mut impl Write) -> io::Result<()> {
        while !self.bytes.is_empty() {
            let written = {
                let (first, second) = self.bytes.as_slices();
                let slice = if first.is_empty() { second } else { first };
                match output.write(slice) {
                    Ok(0) => {
                        return Err(io::Error::new(
                            io::ErrorKind::WriteZero,
                            "AFK queue write returned zero",
                        ));
                    }
                    Ok(written) => written,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            };
            self.bytes.drain(..written);
        }
        Ok(())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::ByteQueue;

    #[test]
    fn enforces_limit_and_flushes_in_order() {
        let mut queue = ByteQueue::new(4);
        assert!(queue.push(b"ab"));
        assert!(queue.push(b"cd"));
        assert!(!queue.push(b"e"));

        let mut output = Vec::new();
        assert!(queue.flush(&mut output).is_ok());
        assert_eq!(output, b"abcd");
        assert!(queue.is_empty());
    }
}
