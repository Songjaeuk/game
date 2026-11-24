//! 아주 작은 MPQ 읽기 모듈 (스타2 .s2ma 용). 읽기 전용.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::OnceLock;

fn table() -> &'static [u32; 1280] {
    static T: OnceLock<[u32; 1280]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u32; 1280];
        let mut seed: u32 = 0x0010_0001;
        for i in 0..256 {
            let mut j = i;
            for _ in 0..5 {
                seed = (seed.wrapping_mul(125).wrapping_add(3)) % 0x2A_AAAB;
                let t1 = (seed & 0xFFFF) << 16;
                seed = (seed.wrapping_mul(125).wrapping_add(3)) % 0x2A_AAAB;
                t[j] = t1 | (seed & 0xFFFF);
                j += 256;
            }
        }
        t
    })
}

fn hash(s: &str, kind: usize) -> u32 {
    let t = table();
    let (mut s1, mut s2) = (0x7FED_7FEDu32, 0xEEEE_EEEEu32);
    for ch in s.to_ascii_uppercase().bytes() {
        s1 = t[kind * 256 + ch as usize] ^ s1.wrapping_add(s2);
        s2 = (ch as u32)
            .wrapping_add(s1)
            .wrapping_add(s2)
            .wrapping_add(s2 << 5)
            .wrapping_add(3);
    }
    s1
}

fn decrypt(data: &[u8], mut key: u32) -> Vec<u32> {
    let t = table();
    let mut s2 = 0xEEEE_EEEEu32;
    let mut out = Vec::with_capacity(data.len() / 4);
    for c in data.chunks_exact(4) {
        s2 = s2.wrapping_add(t[0x400 + (key & 0xFF) as usize]);
        let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]) ^ key.wrapping_add(s2);
        out.push(v);
        key = ((!key << 0x15).wrapping_add(0x1111_1111)) | (key >> 0x0B);
        s2 = v.wrapping_add(s2).wrapping_add(s2 << 5).wrapping_add(3);
    }
    out
}

fn read_at(f: &mut File, pos: u64, len: usize) -> Option<Vec<u8>> {
    let mut b = vec![0u8; len];
    f.seek(SeekFrom::Start(pos)).ok()?;
    f.read_exact(&mut b).ok()?;
    Some(b)
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub struct Archive {
    f: File,
    base: u64,
    shift: u16,
    ht: Vec<u32>,
    bt: Vec<u32>,
}

impl Archive {
    pub fn open(path: &std::path::Path) -> Option<Archive> {
        let mut f = File::open(path).ok()?;
        if f.metadata().ok()?.len() < 64 {
            return None;
        }
        let mut base = 0u64;
        let mut h = read_at(&mut f, 0, 32)?;
        if &h[0..4] == b"MPQ\x1B" {
            base = u32le(&h, 8) as u64;
            h = read_at(&mut f, base, 32)?;
        }
        if &h[0..4] != b"MPQ\x1A" {
            return None;
        }
        let shift = u16le(&h, 14);
        let (hto, bto, hn, bn) = (u32le(&h, 16), u32le(&h, 20), u32le(&h, 24), u32le(&h, 28));
        if hn == 0 || hn > (1 << 20) || bn > (1 << 20) || !hn.is_power_of_two() {
            return None;
        }
        let ht = decrypt(&read_at(&mut f, base + hto as u64, hn as usize * 16)?, hash("(hash table)", 3));
        let bt = decrypt(&read_at(&mut f, base + bto as u64, bn as usize * 16)?, hash("(block table)", 3));
        Some(Archive { f, base, shift, ht, bt })
    }

    fn block_index(&self, name: &str) -> Option<usize> {
        let hn = (self.ht.len() / 4) as u32;
        let (a, b) = (hash(name, 1), hash(name, 2));
        let mut i = hash(name, 0) & (hn - 1);
        for _ in 0..hn {
            let e = (i * 4) as usize;
            if self.ht[e + 3] == 0xFFFF_FFFF {
                return None;
            }
            if self.ht[e] == a && self.ht[e + 1] == b && self.ht[e + 3] != 0xFFFF_FFFE {
                return Some(self.ht[e + 3] as usize);
            }
            i = (i + 1) & (hn - 1);
        }
        None
    }

    /// 파일 목록에 이름이 있는지 (압축 방식 상관없음)
    pub fn has(&self, name: &str) -> bool {
        self.block_index(name).is_some()
    }

    /// 파일 내용 읽기 (zlib 또는 무압축만 지원)
    pub fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        let bi = self.block_index(name)?;
        if bi * 4 + 3 >= self.bt.len() {
            return None;
        }
        let (off, asz, size, flags) = (self.bt[bi * 4], self.bt[bi * 4 + 1], self.bt[bi * 4 + 2], self.bt[bi * 4 + 3]);
        if flags & 0x8000_0000 == 0 || flags & 0x1_0000 != 0 || size > 16 * 1024 * 1024 {
            return None;
        }
        let raw = read_at(&mut self.f, self.base + off as u64, asz as usize)?;
        let comp = flags & 0x200 != 0;
        if flags & 0x0100_0000 != 0 {
            return if comp { unpack(&raw, size as usize) } else { Some(raw) };
        }
        if !comp {
            return Some(raw.get(..size as usize)?.to_vec());
        }
        let ss = 512usize << self.shift;
        let ns = (size as usize).div_ceil(ss);
        let mut out = Vec::with_capacity(size as usize);
        for k in 0..ns {
            let s = u32le(&raw, k * 4) as usize;
            let e = u32le(&raw, k * 4 + 4) as usize;
            let want = ss.min(size as usize - k * ss);
            out.extend(unpack(raw.get(s..e)?, want)?);
        }
        Some(out)
    }
}

fn unpack(c: &[u8], size: usize) -> Option<Vec<u8>> {
    if c.len() >= size {
        return Some(c[..size].to_vec());
    }
    if c.first()? != &0x02 {
        return None; // zlib 외 압축은 건너뜀
    }
    let mut d = flate2::read::ZlibDecoder::new(&c[1..]);
    let mut o = Vec::with_capacity(size);
    d.read_to_end(&mut o).ok()?;
    Some(o)
}
