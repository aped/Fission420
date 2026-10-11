//! Minimal ZIP container (PKWARE APPNOTE 6.3.x, sections 4.3-4.4) for 3MF/OPC
//! packages: a bounded reader of stored/deflated entries and a deterministic
//! writer. Not a general archiver: no ZIP64, encryption, multi-disk archives or
//! data descriptors on write.
//!
//! Reader bounds: the archive size, the entry count and every entry's
//! uncompressed size are checked against [`ZipLimits`] before allocation; the
//! inflater stops at the declared size (zip bombs cannot allocate more), and
//! the result must match the declared size and CRC-32 exactly.
use crate::{ExchangeError, Result, UnsupportedEntities};

const FORMAT: &str = "3MF";
const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_END: u32 = 0x0605_4b50;
/// End-of-central-directory record without comment.
const END_LEN: usize = 22;
/// Fixed part of a central directory header.
const CENTRAL_LEN: usize = 46;
/// Fixed part of a local file header.
const LOCAL_LEN: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZipLimits {
    pub max_archive_bytes: usize,
    pub max_entries: usize,
    /// Per entry, uncompressed.
    pub max_entry_bytes: usize,
}

/// One central-directory entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    method: u16,
    flags: u16,
    crc32: u32,
    compressed: u64,
    pub uncompressed: u64,
    local_offset: u64,
}

pub struct ZipReader<'a> {
    data: &'a [u8],
    pub entries: Vec<ZipEntry>,
    limits: ZipLimits,
}

fn truncated(msg: impl Into<String>) -> ExchangeError {
    ExchangeError::Truncated {
        format: FORMAT.into(),
        message: msg.into(),
    }
}

fn malformed(msg: impl Into<String>) -> ExchangeError {
    ExchangeError::Malformed {
        format: FORMAT.into(),
        line: None,
        message: msg.into(),
    }
}

pub(crate) fn unsupported(what: &str, reason: impl Into<String>) -> ExchangeError {
    ExchangeError::Unsupported {
        format: FORMAT.into(),
        entities: vec![UnsupportedEntities {
            entity_type: what.into(),
            count: 1,
            reason: reason.into(),
            first_line: 0,
        }],
    }
}

fn limit(what: &str, limit: usize, actual: u64) -> ExchangeError {
    ExchangeError::LimitExceeded {
        format: FORMAT.into(),
        what: what.into(),
        limit: limit as u64,
        actual,
    }
}

fn u16_at(d: &[u8], at: usize) -> Result<u16> {
    d.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| truncated(format!("archive ends inside a header at byte {at}")))
}

fn u32_at(d: &[u8], at: usize) -> Result<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| truncated(format!("archive ends inside a header at byte {at}")))
}

impl<'a> ZipReader<'a> {
    pub fn new(data: &'a [u8], limits: ZipLimits) -> Result<Self> {
        if data.len() > limits.max_archive_bytes {
            return Err(limit("bytes", limits.max_archive_bytes, data.len() as u64));
        }
        if data.len() < END_LEN {
            return Err(truncated(format!(
                "{} bytes is too short for a ZIP archive",
                data.len()
            )));
        }
        // The end record is the last 22 bytes plus an optional comment (< 64 KiB):
        // scan backwards for its signature with a consistent comment length.
        let lowest = data.len().saturating_sub(END_LEN + u16::MAX as usize);
        let mut end = None;
        for at in (lowest..=data.len() - END_LEN).rev() {
            if u32_at(data, at)? == SIG_END
                && at + END_LEN + u16_at(data, at + 20)? as usize == data.len()
            {
                end = Some(at);
                break;
            }
        }
        let end = end.ok_or_else(|| {
            truncated("no ZIP end-of-central-directory record (not a ZIP/3MF file, or cut short)")
        })?;
        let disk = u16_at(data, end + 4)?;
        let cd_disk = u16_at(data, end + 6)?;
        let n_disk = u16_at(data, end + 8)?;
        let n = u16_at(data, end + 10)?;
        let cd_size = u32_at(data, end + 12)?;
        let cd_off = u32_at(data, end + 16)?;
        if disk != 0 || cd_disk != 0 || n_disk != n {
            return Err(unsupported(
                "multi-disk ZIP",
                "split archives are not supported",
            ));
        }
        if n == u16::MAX || cd_size == u32::MAX || cd_off == u32::MAX {
            return Err(unsupported("ZIP64", "ZIP64 archives are not supported"));
        }
        if n as usize > limits.max_entries {
            return Err(limit("archive entries", limits.max_entries, n as u64));
        }
        let (cd_off, cd_size) = (cd_off as usize, cd_size as usize);
        if cd_off.checked_add(cd_size).is_none_or(|e| e > end) {
            return Err(malformed("central directory lies outside the archive"));
        }
        let mut entries = Vec::with_capacity(n as usize);
        let mut at = cd_off;
        for _ in 0..n {
            if u32_at(data, at)? != SIG_CENTRAL {
                return Err(malformed(format!(
                    "bad central directory header signature at byte {at}"
                )));
            }
            let flags = u16_at(data, at + 8)?;
            let method = u16_at(data, at + 10)?;
            let crc32 = u32_at(data, at + 16)?;
            let compressed = u32_at(data, at + 20)?;
            let uncompressed = u32_at(data, at + 24)?;
            let name_len = u16_at(data, at + 28)? as usize;
            let extra_len = u16_at(data, at + 30)? as usize;
            let comment_len = u16_at(data, at + 32)? as usize;
            let local = u32_at(data, at + 42)?;
            if compressed == u32::MAX || uncompressed == u32::MAX || local == u32::MAX {
                return Err(unsupported("ZIP64", "ZIP64 entries are not supported"));
            }
            let name_at = at + CENTRAL_LEN;
            let name = data
                .get(name_at..name_at + name_len)
                .ok_or_else(|| truncated("archive ends inside an entry name"))?;
            let name = std::str::from_utf8(name)
                .map_err(|_| malformed("entry name is not UTF-8"))?
                .to_string();
            entries.push(ZipEntry {
                name,
                method,
                flags,
                crc32,
                compressed: compressed as u64,
                uncompressed: uncompressed as u64,
                local_offset: local as u64,
            });
            at = name_at + name_len + extra_len + comment_len;
            if at > cd_off + cd_size {
                return Err(malformed("central directory entry overruns the directory"));
            }
        }
        Ok(Self {
            data,
            entries,
            limits,
        })
    }

    /// Entry by OPC part name: ASCII case-insensitive, leading '/' ignored.
    pub fn find(&self, part: &str) -> Option<&ZipEntry> {
        let part = part.trim_start_matches('/');
        self.entries
            .iter()
            .find(|e| e.name.trim_start_matches('/').eq_ignore_ascii_case(part))
    }

    /// Bounded, verified contents of `e`.
    pub fn read(&self, e: &ZipEntry) -> Result<Vec<u8>> {
        if e.flags & 1 != 0 {
            return Err(unsupported(
                "encrypted ZIP entry",
                format!("{} is encrypted", e.name),
            ));
        }
        if e.uncompressed > self.limits.max_entry_bytes as u64 {
            return Err(limit(
                &format!("uncompressed bytes in {}", e.name),
                self.limits.max_entry_bytes,
                e.uncompressed,
            ));
        }
        let at = e.local_offset as usize;
        if u32_at(self.data, at)? != SIG_LOCAL {
            return Err(malformed(format!(
                "bad local header signature for {} at byte {at}",
                e.name
            )));
        }
        let name_len = u16_at(self.data, at + 26)? as usize;
        let extra_len = u16_at(self.data, at + 28)? as usize;
        let start = at + LOCAL_LEN + name_len + extra_len;
        let raw = start
            .checked_add(e.compressed as usize)
            .and_then(|end| self.data.get(start..end))
            .ok_or_else(|| truncated(format!("archive ends inside the data of {}", e.name)))?;
        let out = match e.method {
            0 => raw.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, e.uncompressed as usize)
                .map_err(|err| {
                malformed(format!(
                    "{}: deflate data is corrupt or larger than declared ({:?})",
                    e.name, err.status
                ))
            })?,
            m => {
                return Err(unsupported(
                    "ZIP compression method",
                    format!(
                        "{} uses method {m}; only stored (0) and deflate (8) are supported",
                        e.name
                    ),
                ));
            }
        };
        if out.len() as u64 != e.uncompressed {
            return Err(malformed(format!(
                "{}: {} bytes after decompression, {} declared",
                e.name,
                out.len(),
                e.uncompressed
            )));
        }
        if crc32fast::hash(&out) != e.crc32 {
            return Err(malformed(format!("{}: CRC-32 mismatch", e.name)));
        }
        Ok(out)
    }
}

/// Deterministic archive writer: deflated entries (stored when deflate does
/// not help), fixed DOS timestamp 1980-01-01 00:00, no extra fields.
#[derive(Default)]
pub struct ZipWriter {
    out: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

impl ZipWriter {
    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<()> {
        let too_big = |what: &str| {
            ExchangeError::InvalidOption(format!("{what} too large for a non-ZIP64 archive"))
        };
        if self.count == u16::MAX - 1 {
            return Err(too_big("entry count"));
        }
        let deflated = miniz_oxide::deflate::compress_to_vec(data, 6);
        let (method, body): (u16, &[u8]) = if deflated.len() < data.len() {
            (8, &deflated)
        } else {
            (0, data)
        };
        let crc = crc32fast::hash(data);
        let size = u32::try_from(data.len()).map_err(|_| too_big(name))?;
        let csize = u32::try_from(body.len()).map_err(|_| too_big(name))?;
        let offset = u32::try_from(self.out.len()).map_err(|_| too_big("archive"))?;
        let name_len = u16::try_from(name.len()).map_err(|_| too_big("entry name"))?;
        // Version needed 2.0; general purpose flag bit 11: UTF-8 names.
        let common = |v: &mut Vec<u8>| {
            v.extend_from_slice(&20u16.to_le_bytes());
            v.extend_from_slice(&0x0800u16.to_le_bytes());
            v.extend_from_slice(&method.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes()); // time 00:00:00
            v.extend_from_slice(&0x0021u16.to_le_bytes()); // date 1980-01-01
            v.extend_from_slice(&crc.to_le_bytes());
            v.extend_from_slice(&csize.to_le_bytes());
            v.extend_from_slice(&size.to_le_bytes());
            v.extend_from_slice(&name_len.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes()); // extra length
        };
        self.out.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        common(&mut self.out);
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(body);
        self.central.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
        self.central.extend_from_slice(&20u16.to_le_bytes()); // made by: 2.0, MS-DOS
        common(&mut self.central);
        self.central.extend_from_slice(&0u16.to_le_bytes()); // comment length
        self.central.extend_from_slice(&0u16.to_le_bytes()); // disk number
        self.central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        self.central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        self.central.extend_from_slice(&offset.to_le_bytes());
        self.central.extend_from_slice(name.as_bytes());
        self.count += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<Vec<u8>> {
        let too_big =
            || ExchangeError::InvalidOption("archive too large for a non-ZIP64 archive".into());
        let cd_off = u32::try_from(self.out.len()).map_err(|_| too_big())?;
        let cd_size = u32::try_from(self.central.len()).map_err(|_| too_big())?;
        self.out.extend_from_slice(&self.central);
        self.out.extend_from_slice(&SIG_END.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&cd_size.to_le_bytes());
        self.out.extend_from_slice(&cd_off.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIM: ZipLimits = ZipLimits {
        max_archive_bytes: 1 << 20,
        max_entries: 16,
        max_entry_bytes: 1 << 20,
    };

    #[test]
    fn write_then_read_stored_and_deflated() {
        let mut w = ZipWriter::default();
        w.add("a.txt", b"x").unwrap(); // stored: deflate does not help
        let big = vec![b'a'; 10_000];
        w.add("dir/B.bin", &big).unwrap();
        let z = w.finish().unwrap();
        let r = ZipReader::new(&z, LIM).unwrap();
        assert_eq!(r.entries.len(), 2);
        assert_eq!(r.read(r.find("/A.TXT").unwrap()).unwrap(), b"x");
        assert_eq!(r.read(r.find("dir/b.bin").unwrap()).unwrap(), big);
    }

    #[test]
    fn every_truncation_and_bit_flip_is_an_error_not_a_panic() {
        let mut w = ZipWriter::default();
        w.add("m", &vec![7u8; 3000]).unwrap();
        let z = w.finish().unwrap();
        for n in 0..z.len() {
            let r = ZipReader::new(&z[..n], LIM).and_then(|r| r.read(&r.entries[0].clone()));
            assert!(r.is_err(), "prefix {n}");
        }
        for i in 0..z.len() {
            let mut c = z.clone();
            c[i] ^= 0x5a;
            // Must not panic; most flips are detected (CRC, sizes, signatures).
            let _ = ZipReader::new(&c, LIM).and_then(|r| {
                let e = r
                    .entries
                    .first()
                    .cloned()
                    .ok_or_else(|| truncated("none"))?;
                r.read(&e)
            });
        }
    }

    #[test]
    fn declared_size_over_the_limit_is_refused_before_inflating() {
        let mut w = ZipWriter::default();
        w.add("bomb", &vec![0u8; 200_000]).unwrap();
        let z = w.finish().unwrap();
        let small = ZipLimits {
            max_entry_bytes: 100_000,
            ..LIM
        };
        let r = ZipReader::new(&z, small).unwrap();
        let e = r.entries[0].clone();
        assert!(matches!(
            r.read(&e),
            Err(ExchangeError::LimitExceeded { .. })
        ));
    }
}
