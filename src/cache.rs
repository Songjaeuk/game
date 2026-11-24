//! 배틀넷 캐시 읽기: 맵/모드 목록, 의존 모드 찾기. 파일은 읽기만 함.
use crate::mpq::Archive;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct Dep {
    pub bnet_name: String,
    pub id: String,
    pub file: String, // 원본 파일 이름 (예: Example.SC2Mod)
}

#[derive(Clone, Debug)]
pub struct Doc {
    pub path: PathBuf,
    pub hash: String,
    /// (로캘, 이름) 목록: RKok=한국어, SUne=영어, NChz=중국어(간체), WThz=중국어(번체)
    pub names: Vec<(String, String)>,
    pub is_map: bool,
    pub deps: Vec<Dep>,
    pub size: u64,
    pub mtime: SystemTime,
}

#[derive(Default)]
pub struct Progress {
    pub stage: String,
    pub done: usize,
    pub total: usize,
}

pub struct Cache {
    pub docs: Vec<Doc>,
    mh: Vec<(SystemTime, Vec<u8>)>,
    by_hash: HashMap<String, usize>,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
}

fn ext_is(p: &Path, e: &str) -> bool {
    p.extension().map(|x| x.eq_ignore_ascii_case(e)).unwrap_or(false)
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// DocumentHeader 에서 이름 꺼내기: [u16 12]"DocInfo/Name"[로캘 4][u16 길이][값]
fn header_names(h: &[u8]) -> Vec<(String, String)> {
    let key = b"\x0c\x00DocInfo/Name";
    let mut out: Vec<(String, String)> = Vec::new();
    let mut at = 0;
    while let Some(i) = find(h, key, at) {
        at = i + 1;
        let p = i + key.len();
        if p + 6 > h.len() {
            break;
        }
        let loc = String::from_utf8_lossy(&h[p..p + 4]).to_string();
        let len = u16::from_le_bytes([h[p + 4], h[p + 5]]) as usize;
        let Some(v) = h.get(p + 6..p + 6 + len) else { continue };
        let val = String::from_utf8_lossy(v).trim().to_string();
        if !val.is_empty() && !out.iter().any(|(l, _)| *l == loc) {
            out.push((loc, val));
        }
    }
    out
}

impl Doc {
    /// 원하는 로캘 순서대로 이름 고르기
    pub fn name(&self, prefer: &[&str]) -> String {
        for p in prefer {
            if let Some((_, n)) = self.names.iter().find(|(l, _)| l == p) {
                return n.clone();
            }
        }
        self.names.first().map(|(_, n)| n.clone()).unwrap_or_else(|| "?".into())
    }
    pub fn matches(&self, q: &str) -> bool {
        self.names.iter().any(|(_, n)| n.to_lowercase().contains(q))
    }
}

/// "bnet:이름/버전/번호,file:경로" 목록 (번호 999 = 블리자드 기본, 제외)
fn header_deps(h: &[u8]) -> Vec<Dep> {
    let mut v = Vec::new();
    let mut at = 0;
    while let Some(i) = find(h, b"bnet:", at) {
        let end = h[i..].iter().position(|&c| c == 0).map(|n| i + n).unwrap_or(h.len());
        at = end;
        let s = String::from_utf8_lossy(&h[i + 5..end]).to_string();
        let (left, file) = match s.split_once(",file:") {
            Some((l, f)) => (l.to_string(), f.rsplit(['\\', '/']).next().unwrap_or("").to_string()),
            None => (s.clone(), String::new()),
        };
        let parts: Vec<&str> = left.rsplitn(3, '/').collect(); // [번호, 버전, 이름]
        if parts.len() < 3 || parts[0] == "999" {
            continue;
        }
        v.push(Dep { bnet_name: parts[2].to_string(), id: parts[0].to_string(), file });
    }
    v
}

impl Cache {
    pub fn load(root: &Path, prog: &Arc<Mutex<Progress>>) -> Cache {
        let set = |stage: &str, done: usize, total: usize| {
            let mut p = prog.lock().unwrap();
            p.stage = stage.into();
            p.done = done;
            p.total = total;
        };
        set("list", 0, 0);
        let mut all = Vec::new();
        walk(root, &mut all);

        let archives: Vec<&PathBuf> = all.iter().filter(|p| ext_is(p, "s2ma") || ext_is(p, "s2mod")).collect();
        let mut docs = Vec::new();
        for (n, p) in archives.iter().enumerate() {
            set("archives", n + 1, archives.len());
            let Some(mut a) = Archive::open(p) else { continue };
            let Some(h) = a.read("DocumentHeader") else { continue };
            let meta = std::fs::metadata(p).ok();
            docs.push(Doc {
                path: (*p).clone(),
                hash: p.file_stem().unwrap_or_default().to_string_lossy().to_lowercase(),
                names: header_names(&h),
                is_map: a.has("MapInfo"),
                deps: header_deps(&h),
                size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                mtime: meta.and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }

        let mhs: Vec<&PathBuf> = all.iter().filter(|p| ext_is(p, "s2mh")).collect();
        let mut mh = Vec::new();
        for (n, p) in mhs.iter().enumerate() {
            if n % 200 == 0 {
                set("info", n + 1, mhs.len());
            }
            let Ok(m) = std::fs::metadata(p) else { continue };
            if m.len() > 2 * 1024 * 1024 {
                continue;
            }
            if let Ok(d) = std::fs::read(p) {
                mh.push((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), d));
            }
        }
        mh.sort_by(|a, b| b.0.cmp(&a.0));

        let by_hash = docs.iter().enumerate().map(|(i, d)| (d.hash.clone(), i)).collect();
        set("done", 1, 1);
        Cache { docs, mh, by_hash }
    }

    /// 원본 파일 이름 → 캐시 모드 후보 (최신 순)
    fn by_file_name(&self, file: &str) -> Vec<usize> {
        let nb = file.as_bytes();
        if nb.is_empty() || nb.len() > 127 {
            return vec![];
        }
        let mut needle = vec![(nb.len() * 2) as u8];
        needle.extend_from_slice(nb);
        let mut hits: Vec<usize> = Vec::new();
        for (_, d) in &self.mh {
            let Some(at) = find(d, &needle, 0) else { continue };
            let Some(t) = find(d, b"s2ma", at) else { continue };
            let Some(hb) = d.get(t + 8..t + 40) else { continue };
            let hx: String = hb.iter().map(|b| format!("{b:02x}")).collect();
            if let Some(&i) = self.by_hash.get(&hx) {
                if !self.docs[i].is_map && !hits.contains(&i) {
                    hits.push(i);
                }
            }
        }
        hits.sort_by(|a, b| self.docs[*b].mtime.cmp(&self.docs[*a].mtime));
        hits
    }

    /// 배틀넷 번호 → 캐시 모드 후보 (최신 순)
    /// .s2mh 맨 앞부분에 번호가 [0x09][varint(번호*2)] 로 적혀 있음 (예: 190748 → 09 b8 a4 17)
    fn by_id(&self, id: &str) -> Vec<usize> {
        let Ok(n) = id.parse::<u64>() else { return vec![] };
        let mut v = n << 1;
        let mut needle = vec![0x09u8];
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                needle.push(b);
                break;
            }
            needle.push(b | 0x80);
        }
        let mut hits: Vec<usize> = Vec::new();
        for (_, d) in &self.mh {
            let head = &d[..d.len().min(48)];
            if find(head, &needle, 0).is_none() {
                continue;
            }
            let Some(t) = find(d, b"s2ma", 0) else { continue };
            let Some(hb) = d.get(t + 8..t + 40) else { continue };
            let hx: String = hb.iter().map(|b| format!("{b:02x}")).collect();
            if let Some(&i) = self.by_hash.get(&hx) {
                if !self.docs[i].is_map && !hits.contains(&i) {
                    hits.push(i);
                }
            }
        }
        hits.sort_by(|a, b| self.docs[*b].mtime.cmp(&self.docs[*a].mtime));
        hits
    }

    /// 맵이 쓰는 모드 찾기 (모드가 쓰는 모드까지)
    pub fn resolve(&self, map: usize) -> Vec<(Dep, Option<usize>, usize)> {
        let mut out = Vec::new();
        let mut queue: Vec<Dep> = self.docs[map].deps.clone();
        let mut seen = std::collections::HashSet::new();
        while !queue.is_empty() {
            let dep = queue.remove(0);
            if !seen.insert(dep.id.clone()) {
                continue;
            }
            let mut hits = self.by_file_name(&dep.file);
            if hits.is_empty() {
                hits = self.by_id(&dep.id);
            }
            if hits.is_empty() {
                hits = (0..self.docs.len())
                    .filter(|&i| !self.docs[i].is_map && self.docs[i].names.iter().any(|(_, n)| *n == dep.bnet_name))
                    .collect();
                hits.sort_by(|a, b| self.docs[*b].mtime.cmp(&self.docs[*a].mtime));
            }
            if let Some(&f) = hits.first() {
                queue.extend(self.docs[f].deps.clone());
            }
            let n = hits.len();
            out.push((dep, hits.first().copied(), n));
        }
        out
    }
}

