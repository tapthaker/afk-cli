use std::collections::VecDeque;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const TRUNCATION_MARKER: &[u8] = b"\r\n[afk: earlier terminal output was truncated]\r\n";

pub(crate) struct OutputTail {
    bytes: VecDeque<u8>,
    capacity: usize,
    truncated: bool,
}

impl OutputTail {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(capacity),
            capacity,
            truncated: false,
        }
    }

    pub(crate) fn extend(&mut self, incoming: &[u8]) {
        if incoming.is_empty() {
            return;
        }
        if self.capacity == 0 {
            self.truncated = true;
            return;
        }
        if incoming.len() >= self.capacity {
            self.truncated |= !self.bytes.is_empty() || incoming.len() > self.capacity;
            self.bytes.clear();
            self.bytes
                .extend(incoming[incoming.len() - self.capacity..].iter().copied());
            return;
        }

        let overflow = self
            .bytes
            .len()
            .saturating_add(incoming.len())
            .saturating_sub(self.capacity);
        if overflow != 0 {
            self.bytes.drain(..overflow);
            self.truncated = true;
        }
        self.bytes.extend(incoming.iter().copied());
    }

    pub(crate) fn snapshot(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn is_truncated(&self) -> bool {
        self.truncated
    }
}

#[cfg(test)]
mod tests {
    use super::OutputTail;

    #[test]
    fn retains_all_bytes_until_capacity() {
        let mut tail = OutputTail::new(6);
        tail.extend(b"abc");
        tail.extend(b"def");

        assert_eq!(tail.snapshot(), b"abcdef");
        assert!(!tail.is_truncated());
    }

    #[test]
    fn retains_exact_final_bytes_after_wrapping() {
        let mut tail = OutputTail::new(6);
        tail.extend(b"abcd");
        tail.extend(b"efgh");

        assert_eq!(tail.snapshot(), b"cdefgh");
        assert_eq!(tail.len(), 6);
        assert!(tail.is_truncated());
    }

    #[test]
    fn oversized_single_chunk_retains_only_its_end() {
        let mut tail = OutputTail::new(4);
        tail.extend(b"existing");
        tail.extend(b"0123456789");

        assert_eq!(tail.snapshot(), b"6789");
        assert!(tail.is_truncated());
    }

    #[test]
    fn exact_capacity_on_empty_tail_is_not_truncated() {
        let mut tail = OutputTail::new(4);
        tail.extend(b"0123");

        assert_eq!(tail.snapshot(), b"0123");
        assert!(!tail.is_truncated());
    }
}
