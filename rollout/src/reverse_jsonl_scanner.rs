use std::io;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;

use serde::de::DeserializeOwned;

const READ_CHUNK_SIZE: usize = 8 * 1024;

#[derive(Debug)]
pub enum ScanOutcome<T> {
    /// The record was valid JSON and deserialized as the requested type.
    Parsed(T),
    /// The record was not valid JSON for the requested type.
    Rejected(serde_json::Error),
}

/// Read-only scanner for newline-delimited JSON records, starting from the end.
pub struct ReverseJsonlScanner<R> {
    reader: R,
    next_chunk_end: u64,
    chunk_position: usize,
    chunk: Vec<u8>,
    record_reversed: Vec<u8>,
}

impl<R> ReverseJsonlScanner<R>
where
    R: Read + Seek,
{
    pub fn new(mut reader: R) -> io::Result<Self> {
        let next_chunk_end = reader.seek(SeekFrom::End(0))?;
        Ok(Self {
            reader,
            next_chunk_end,
            chunk_position: 0,
            chunk: vec![0; READ_CHUNK_SIZE],
            record_reversed: Vec::new(),
        })
    }

    /// Scans the next nonblank record.
    ///
    /// I/O failures are returned as [`Err`]. Invalid JSON records are returned as
    /// [`ScanOutcome::Rejected`], and the scanner remains usable.
    pub fn scan_next<T>(&mut self) -> io::Result<Option<ScanOutcome<T>>>
    where
        T: DeserializeOwned,
    {
        loop {
            let Some(byte) = self.read_previous_byte()? else {
                return Ok(self.finish_record());
            };

            if byte != b'\n' {
                self.record_reversed.push(byte);
                continue;
            }

            if let Some(outcome) = self.finish_record() {
                return Ok(Some(outcome));
            }
        }
    }

    fn read_previous_byte(&mut self) -> io::Result<Option<u8>> {
        if self.chunk_position == 0 {
            if self.next_chunk_end == 0 {
                return Ok(None);
            }

            let read_size = usize::try_from(self.next_chunk_end.min(READ_CHUNK_SIZE as u64))
                .map_err(io::Error::other)?;
            self.next_chunk_end -= read_size as u64;
            self.reader.seek(SeekFrom::Start(self.next_chunk_end))?;
            self.reader.read_exact(&mut self.chunk[..read_size])?;
            self.chunk_position = read_size;
        }

        self.chunk_position -= 1;
        Ok(Some(self.chunk[self.chunk_position]))
    }

    fn finish_record<T>(&mut self) -> Option<ScanOutcome<T>>
    where
        T: DeserializeOwned,
    {
        self.record_reversed.reverse();
        let outcome = if self.record_reversed.iter().all(u8::is_ascii_whitespace) {
            None
        } else {
            Some(match serde_json::from_slice::<T>(&self.record_reversed) {
                Ok(value) => ScanOutcome::Parsed(value),
                Err(error) => ScanOutcome::Rejected(error),
            })
        };
        self.record_reversed.clear();
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::ReverseJsonlScanner;
    use super::ScanOutcome;
    use std::io::Cursor;

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct Record {
        id: u32,
    }

    #[test]
    fn scans_records_in_reverse_order() {
        let data = b"{\"id\":1}\n{\"id\":2}\n{\"id\":3}\n";
        let cursor = Cursor::new(data.as_slice());
        let mut scanner = ReverseJsonlScanner::new(cursor).unwrap();

        let mut ids = Vec::new();
        while let Some(outcome) = scanner.scan_next::<Record>().unwrap() {
            match outcome {
                ScanOutcome::Parsed(record) => ids.push(record.id),
                ScanOutcome::Rejected(err) => panic!("unexpected reject: {err}"),
            }
        }
        assert_eq!(ids, vec![3, 2, 1]);
    }

    #[test]
    fn skips_blank_lines() {
        let data = b"{\"id\":1}\n\n\n{\"id\":2}\n";
        let cursor = Cursor::new(data.as_slice());
        let mut scanner = ReverseJsonlScanner::new(cursor).unwrap();

        let first = scanner.scan_next::<Record>().unwrap().unwrap();
        assert!(matches!(first, ScanOutcome::Parsed(Record { id: 2 })));
        let second = scanner.scan_next::<Record>().unwrap().unwrap();
        assert!(matches!(second, ScanOutcome::Parsed(Record { id: 1 })));
        assert!(scanner.scan_next::<Record>().unwrap().is_none());
    }

    #[test]
    fn returns_rejected_for_invalid_json() {
        let data = b"{\"id\":1}\nnot json\n{\"id\":2}\n";
        let cursor = Cursor::new(data.as_slice());
        let mut scanner = ReverseJsonlScanner::new(cursor).unwrap();

        let first = scanner.scan_next::<Record>().unwrap().unwrap();
        assert!(matches!(first, ScanOutcome::Parsed(Record { id: 2 })));
        let second = scanner.scan_next::<Record>().unwrap().unwrap();
        assert!(matches!(second, ScanOutcome::Rejected(_)));
        let third = scanner.scan_next::<Record>().unwrap().unwrap();
        assert!(matches!(third, ScanOutcome::Parsed(Record { id: 1 })));
    }

    #[test]
    fn empty_file_has_no_records() {
        let cursor = Cursor::new(b"");
        let mut scanner = ReverseJsonlScanner::new(cursor).unwrap();
        assert!(scanner.scan_next::<Record>().unwrap().is_none());
    }

    #[test]
    fn no_trailing_newline() {
        let data = b"{\"id\":1}\n{\"id\":2}";
        let cursor = Cursor::new(data.as_slice());
        let mut scanner = ReverseJsonlScanner::new(cursor).unwrap();

        let mut ids = Vec::new();
        while let Some(outcome) = scanner.scan_next::<Record>().unwrap() {
            match outcome {
                ScanOutcome::Parsed(record) => ids.push(record.id),
                ScanOutcome::Rejected(err) => panic!("unexpected reject: {err}"),
            }
        }
        assert_eq!(ids, vec![2, 1]);
    }
}
