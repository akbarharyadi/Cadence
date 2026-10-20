//! Reading FLAC files with a repeated STREAMINFO block.
//!
//! Some taggers write the STREAMINFO metadata block twice. Symphonia rightly refuses such files
//! ("found more than one stream info block") while most players use the first one. This presents
//! the file to the decoder with the extra copies left out; the file on disk is never changed.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use symphonia::core::io::MediaSource;

pub struct PatchedFlac {
    /// "fLaC" plus the metadata blocks we keep.
    head: Vec<u8>,
    file: File,
    /// Where the audio frames start in the real file.
    audio_start: u64,
    len: u64,
    pos: u64,
}

impl PatchedFlac {
    /// Opens `path` if it's a FLAC with more than one STREAMINFO block; None otherwise.
    pub fn open(path: &Path) -> io::Result<Option<Self>> {
        let mut file = File::open(path)?;
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic)?;
        if &magic != b"fLaC" {
            return Ok(None);
        }
        let mut blocks: Vec<(u8, Vec<u8>)> = Vec::new();
        let mut stream_infos = 0;
        loop {
            let mut h = [0u8; 4];
            file.read_exact(&mut h)?;
            let last = h[0] & 0x80 != 0;
            let kind = h[0] & 0x7F;
            let len = u32::from_be_bytes([0, h[1], h[2], h[3]]) as usize;
            let mut body = vec![0u8; len];
            file.read_exact(&mut body)?;
            if kind == 0 {
                stream_infos += 1;
                if stream_infos > 1 {
                    if last {
                        break;
                    }
                    continue;
                }
            }
            blocks.push((kind, body));
            if last {
                break;
            }
        }
        if stream_infos <= 1 {
            return Ok(None);
        }
        let audio_start = file.stream_position()?;
        let mut head = b"fLaC".to_vec();
        let count = blocks.len();
        for (i, (kind, body)) in blocks.into_iter().enumerate() {
            let flag = if i + 1 == count { 0x80 } else { 0 };
            let n = body.len() as u32;
            head.extend_from_slice(&[flag | kind, (n >> 16) as u8, (n >> 8) as u8, n as u8]);
            head.extend_from_slice(&body);
        }
        let total = file.metadata()?.len();
        let len = head.len() as u64 + total - audio_start;
        file.seek(SeekFrom::Start(audio_start))?;
        Ok(Some(Self {
            head,
            file,
            audio_start,
            len,
            pos: 0,
        }))
    }
}

impl Read for PatchedFlac {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let head_len = self.head.len() as u64;
        if self.pos < head_len {
            let start = self.pos as usize;
            let n = buf.len().min(self.head.len() - start);
            buf[..n].copy_from_slice(&self.head[start..start + n]);
            self.pos += n as u64;
            return Ok(n);
        }
        self.file.seek(SeekFrom::Start(self.audio_start + self.pos - head_len))?;
        let n = self.file.read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for PatchedFlac {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::Current(d) => self.pos as i64 + d,
            SeekFrom::End(d) => self.len as i64 + d,
        };
        if target < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = target as u64;
        Ok(self.pos)
    }
}

impl MediaSource for PatchedFlac {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn drops_the_second_stream_info() {
        let block = |flag: u8, kind: u8, body: &[u8]| {
            let n = body.len();
            let mut v = vec![flag | kind, (n >> 16) as u8, (n >> 8) as u8, n as u8];
            v.extend_from_slice(body);
            v
        };
        let mut data = b"fLaC".to_vec();
        data.extend(block(0, 0, &[1; 34]));
        data.extend(block(0, 4, b"tags"));
        data.extend(block(0x80, 0, &[2; 34]));
        data.extend_from_slice(b"AUDIO");
        let path = std::env::temp_dir().join("cadence-flacfix-test.flac");
        std::fs::File::create(&path).unwrap().write_all(&data).unwrap();

        let mut p = PatchedFlac::open(&path).unwrap().expect("needs patching");
        let mut out = Vec::new();
        p.read_to_end(&mut out).unwrap();
        let mut want = b"fLaC".to_vec();
        want.extend(block(0, 0, &[1; 34]));
        want.extend(block(0x80, 4, b"tags"));
        want.extend_from_slice(b"AUDIO");
        assert_eq!(out, want);
        assert_eq!(p.byte_len(), Some(want.len() as u64));
        let _ = std::fs::remove_file(path);
    }
}
