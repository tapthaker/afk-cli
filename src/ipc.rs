use crate::limits::{
    IPC_HEADER_BYTES, MAX_IPC_PAYLOAD_BYTES, MAX_IPC_RECORD_BYTES, MAX_TERMINAL_DIMENSION,
};
use std::io::{self, Write};

const ATTACH: u8 = 1;
const INPUT: u8 = 2;
const OUTPUT: u8 = 3;
const RESIZE: u8 = 4;
const STOP: u8 = 5;
const EXIT: u8 = 6;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Record {
    Attach { rows: u16, columns: u16 },
    Input(Vec<u8>),
    Output(Vec<u8>),
    Resize { rows: u16, columns: u16 },
    Stop,
    Exit(ProcessExit),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessExit {
    Code(u8),
    Signal(u8),
}

impl ProcessExit {
    pub(crate) const fn status_code(self) -> u8 {
        match self {
            Self::Code(code) => code,
            Self::Signal(signal) => 128_u8.saturating_add(signal),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecodeError {
    BufferLimit,
    PayloadTooLarge,
    UnknownKind,
    InvalidPayload,
    #[cfg(test)]
    TrailingData,
    #[cfg(test)]
    Truncated,
}

#[derive(Default)]
pub(crate) struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    pub(crate) fn remaining_capacity(&self) -> usize {
        MAX_IPC_RECORD_BYTES.saturating_sub(self.buffer.len())
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<(), DecodeError> {
        if bytes.len() > self.remaining_capacity() {
            return Err(DecodeError::BufferLimit);
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn next(&mut self) -> Result<Option<Record>, DecodeError> {
        if self.buffer.len() < IPC_HEADER_BYTES {
            return Ok(None);
        }

        let kind = self.buffer[0];
        let payload_length = u32::from_be_bytes([
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
            self.buffer[4],
        ]) as usize;
        if payload_length > MAX_IPC_PAYLOAD_BYTES {
            return Err(DecodeError::PayloadTooLarge);
        }

        let record_length = IPC_HEADER_BYTES + payload_length;
        if self.buffer.len() < record_length {
            return Ok(None);
        }

        let record = decode_payload(kind, &self.buffer[IPC_HEADER_BYTES..record_length])?;
        self.buffer.drain(..record_length);
        Ok(Some(record))
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

#[cfg(test)]
pub(crate) fn decode_exact(bytes: &[u8]) -> Result<Record, DecodeError> {
    let mut decoder = Decoder::default();
    decoder.push(bytes)?;
    let record = decoder.next()?.ok_or(DecodeError::Truncated)?;
    if !decoder.is_empty() {
        return Err(DecodeError::TrailingData);
    }
    Ok(record)
}

pub(crate) fn write_record(output: &mut impl Write, record: &Record) -> io::Result<()> {
    let (kind, payload_length) = encoded_shape(record);
    let payload_length = u32::try_from(payload_length)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "IPC payload too large"))?;
    output.write_all(&[kind])?;
    output.write_all(&payload_length.to_be_bytes())?;

    match record {
        Record::Attach { rows, columns } | Record::Resize { rows, columns } => {
            output.write_all(&rows.to_be_bytes())?;
            output.write_all(&columns.to_be_bytes())
        }
        Record::Input(bytes) | Record::Output(bytes) => {
            if bytes.len() > MAX_IPC_PAYLOAD_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "IPC payload too large",
                ));
            }
            output.write_all(bytes)
        }
        Record::Stop => Ok(()),
        Record::Exit(ProcessExit::Code(code)) => output.write_all(&[0, *code]),
        Record::Exit(ProcessExit::Signal(signal)) => output.write_all(&[1, *signal]),
    }
}

pub(crate) fn encode(record: &Record) -> io::Result<Vec<u8>> {
    let (_, payload_length) = encoded_shape(record);
    if payload_length > MAX_IPC_PAYLOAD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "IPC payload too large",
        ));
    }
    let mut encoded = Vec::with_capacity(IPC_HEADER_BYTES + payload_length);
    write_record(&mut encoded, record)?;
    Ok(encoded)
}

fn encoded_shape(record: &Record) -> (u8, usize) {
    match record {
        Record::Attach { .. } => (ATTACH, 4),
        Record::Input(bytes) => (INPUT, bytes.len()),
        Record::Output(bytes) => (OUTPUT, bytes.len()),
        Record::Resize { .. } => (RESIZE, 4),
        Record::Stop => (STOP, 0),
        Record::Exit(_) => (EXIT, 2),
    }
}

fn decode_payload(kind: u8, payload: &[u8]) -> Result<Record, DecodeError> {
    match kind {
        ATTACH => {
            decode_dimensions(payload).map(|(rows, columns)| Record::Attach { rows, columns })
        }
        INPUT => Ok(Record::Input(payload.to_vec())),
        OUTPUT => Ok(Record::Output(payload.to_vec())),
        RESIZE => {
            decode_dimensions(payload).map(|(rows, columns)| Record::Resize { rows, columns })
        }
        STOP if payload.is_empty() => Ok(Record::Stop),
        STOP => Err(DecodeError::InvalidPayload),
        EXIT => decode_exit(payload).map(Record::Exit),
        _ => Err(DecodeError::UnknownKind),
    }
}

fn decode_dimensions(payload: &[u8]) -> Result<(u16, u16), DecodeError> {
    let [row_high, row_low, column_high, column_low] = payload else {
        return Err(DecodeError::InvalidPayload);
    };
    let rows = u16::from_be_bytes([*row_high, *row_low]);
    let columns = u16::from_be_bytes([*column_high, *column_low]);
    if !(1..=MAX_TERMINAL_DIMENSION).contains(&rows)
        || !(1..=MAX_TERMINAL_DIMENSION).contains(&columns)
    {
        return Err(DecodeError::InvalidPayload);
    }
    Ok((rows, columns))
}

fn decode_exit(payload: &[u8]) -> Result<ProcessExit, DecodeError> {
    let [reason, value] = payload else {
        return Err(DecodeError::InvalidPayload);
    };
    match (*reason, *value) {
        (0, code) => Ok(ProcessExit::Code(code)),
        (1, signal @ 1..=127) => Ok(ProcessExit::Signal(signal)),
        _ => Err(DecodeError::InvalidPayload),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DecodeError, Decoder, EXIT, INPUT, ProcessExit, RESIZE, Record, STOP, decode_exact, encode,
    };
    use crate::limits::{MAX_IPC_PAYLOAD_BYTES, MAX_IPC_RECORD_BYTES};

    fn records() -> Vec<Record> {
        vec![
            Record::Attach {
                rows: 24,
                columns: 80,
            },
            Record::Input(vec![0, 1, 2, 255]),
            Record::Output(vec![3, 4, 5]),
            Record::Resize {
                rows: 50,
                columns: 120,
            },
            Record::Stop,
            Record::Exit(ProcessExit::Code(17)),
            Record::Exit(ProcessExit::Signal(15)),
        ]
    }

    #[test]
    fn every_record_round_trips() {
        for record in records() {
            let encoded = encode(&record);
            assert!(encoded.is_ok());
            let decoded = encoded.ok().and_then(|bytes| decode_exact(&bytes).ok());
            assert_eq!(decoded, Some(record));
        }
    }

    #[test]
    fn every_truncation_is_incomplete_not_accepted() {
        for record in records() {
            let encoded = encode(&record);
            assert!(encoded.is_ok());
            let bytes = encoded.unwrap_or_default();
            for length in 0..bytes.len() {
                assert_eq!(decode_exact(&bytes[..length]), Err(DecodeError::Truncated));
            }
        }
    }

    #[test]
    fn rejects_oversized_unknown_invalid_and_trailing_records() {
        let oversized =
            (u32::try_from(MAX_IPC_PAYLOAD_BYTES).unwrap_or_default() + 1).to_be_bytes();
        assert_eq!(
            decode_exact(&[
                INPUT,
                oversized[0],
                oversized[1],
                oversized[2],
                oversized[3]
            ]),
            Err(DecodeError::PayloadTooLarge)
        );
        assert_eq!(
            decode_exact(&[99, 0, 0, 0, 0]),
            Err(DecodeError::UnknownKind)
        );
        assert_eq!(
            decode_exact(&[STOP, 0, 0, 0, 1, 0]),
            Err(DecodeError::InvalidPayload)
        );
        assert_eq!(
            decode_exact(&[RESIZE, 0, 0, 0, 4, 0, 0, 0, 80]),
            Err(DecodeError::InvalidPayload)
        );
        assert_eq!(
            decode_exact(&[EXIT, 0, 0, 0, 2, 1, 0]),
            Err(DecodeError::InvalidPayload)
        );
        assert_eq!(
            decode_exact(&[STOP, 0, 0, 0, 0, 0]),
            Err(DecodeError::TrailingData)
        );
    }

    #[test]
    fn process_status_mapping_is_bounded() {
        assert_eq!(ProcessExit::Code(17).status_code(), 17);
        assert_eq!(ProcessExit::Signal(15).status_code(), 143);
        assert_eq!(ProcessExit::Signal(127).status_code(), 255);
    }

    #[test]
    fn decoder_never_buffers_above_one_record() {
        let mut decoder = Decoder::default();
        assert!(decoder.push(&vec![0; MAX_IPC_RECORD_BYTES]).is_ok());
        assert_eq!(decoder.remaining_capacity(), 0);
        assert_eq!(decoder.push(&[0]), Err(DecodeError::BufferLimit));
    }
}
