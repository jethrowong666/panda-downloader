//! 分段调度器：IDM 式的"动态分段"。
//!
//! 文件被切成若干段，每个连接负责一段。某个连接下完自己的段后，不会闲着，
//! 而是找到"剩余最多"的那一段，从它剩余部分的中点劈开，接手后半段。
//! 这样到最后所有连接几乎同时结束，不会出现"99% 卡在一个慢连接上"的情况。
//!
//! 并发安全的关键：worker 写数据分两步——先 `claim` 拿到允许写的长度，写完再 `commit`。
//! 劈段时的中点至少离当前进度 `min_split` 字节，而单次写入不超过 `MAX_CHUNK`，
//! 只要 `min_split > MAX_CHUNK`，正在写的数据就不会落进被劈走的那半段。

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 单次写盘的最大字节数。网络数据块比这大时会被切小。
pub const MAX_CHUNK: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    /// 本段起点（只用于展示）。
    pub start: u64,
    /// 已经写入到的位置。
    pub pos: u64,
    /// 结束位置（不含）。可能被其他连接劈段而变小。
    pub end: u64,
    #[serde(skip)]
    pub active: bool,
}

impl Segment {
    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.pos)
    }
}

/// 分配给 worker 的一项工作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Job {
    pub idx: usize,
    pub pos: u64,
    pub end: u64,
}

pub struct Scheduler {
    segs: Mutex<Vec<Segment>>,
    min_split: u64,
}

impl Scheduler {
    /// 把 `[0, total)` 平均分成 `parts` 段。
    pub fn new(total: u64, parts: usize, min_split: u64) -> Self {
        Self::new_range(0, total, parts, min_split)
    }

    /// 把 `[from, total)` 平均分成 `parts` 段（`[0, from)` 视为已下载）。
    /// 用于"上次是单连接下到一半，这次服务器支持分段了"的情况。
    pub fn new_range(from: u64, total: u64, parts: usize, min_split: u64) -> Self {
        let min_split = min_split.max(MAX_CHUNK as u64 * 2);
        let parts = parts.max(1) as u64;
        let mut segs = Vec::new();
        let step = total.saturating_sub(from).div_ceil(parts).max(1);
        let mut start = from;
        while start < total {
            let end = (start + step).min(total);
            segs.push(Segment { start, pos: start, end, active: false });
            start = end;
        }
        Self { segs: Mutex::new(segs), min_split }
    }

    /// 从断点记录恢复。
    pub fn from_segments(segs: Vec<Segment>, min_split: u64) -> Self {
        let segs = segs.into_iter().map(|s| Segment { active: false, ..s }).collect();
        Self { segs: Mutex::new(segs), min_split: min_split.max(MAX_CHUNK as u64 * 2) }
    }

    /// 领取下一项工作：优先领没人负责的段，没有就去劈最大的那段。
    pub fn next_job(&self) -> Option<Job> {
        let mut segs = self.segs.lock().unwrap();
        if let Some((idx, s)) = segs.iter_mut().enumerate().find(|(_, s)| !s.active && s.remaining() > 0) {
            s.active = true;
            return Some(Job { idx, pos: s.pos, end: s.end });
        }
        let (idx, rem) = segs
            .iter()
            .enumerate()
            .filter(|(_, s)| s.active)
            .map(|(i, s)| (i, s.remaining()))
            .max_by_key(|&(_, r)| r)?;
        if rem < self.min_split * 2 {
            return None;
        }
        let victim = &mut segs[idx];
        let mid = victim.pos + rem / 2;
        let end = victim.end;
        victim.end = mid;
        segs.push(Segment { start: mid, pos: mid, end, active: true });
        Some(Job { idx: segs.len() - 1, pos: mid, end })
    }

    /// 第一步：申请写入 `want` 字节，返回实际允许写的长度（段被劈短后可能变小，0 表示该停了）。
    pub fn claim(&self, idx: usize, want: usize) -> usize {
        let segs = self.segs.lock().unwrap();
        let s = &segs[idx];
        (s.remaining().min(want as u64)) as usize
    }

    /// 第二步：写完后推进进度。
    pub fn commit(&self, idx: usize, n: usize) {
        let mut segs = self.segs.lock().unwrap();
        segs[idx].pos += n as u64;
    }

    /// 当前段的结束位置。
    pub fn end_of(&self, idx: usize) -> u64 {
        self.segs.lock().unwrap()[idx].end
    }

    pub fn pos_of(&self, idx: usize) -> u64 {
        self.segs.lock().unwrap()[idx].pos
    }

    /// 放弃这一段（出错时调用），让其他 worker 可以接手。
    pub fn release(&self, idx: usize) {
        self.segs.lock().unwrap()[idx].active = false;
    }

    pub fn is_done(&self) -> bool {
        self.segs.lock().unwrap().iter().all(|s| s.remaining() == 0)
    }

    /// 所有段剩余字节之和。已下载量 = 总大小 - 剩余（断点恢复时已完成的段不在记录里）。
    pub fn remaining(&self) -> u64 {
        self.segs.lock().unwrap().iter().map(Segment::remaining).sum()
    }

    pub fn active_count(&self) -> usize {
        self.segs.lock().unwrap().iter().filter(|s| s.active && s.remaining() > 0).count()
    }

    /// 用于保存断点。已完成的段会被省略，减小记录体积。
    pub fn snapshot(&self) -> Vec<Segment> {
        let segs = self.segs.lock().unwrap();
        segs.iter().filter(|s| s.remaining() > 0).copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn initial_split_covers_everything() {
        let s = Scheduler::new(10 * MB + 3, 4, MB);
        let segs = s.snapshot();
        assert_eq!(segs.len(), 4);
        assert_eq!(segs[0].start, 0);
        assert_eq!(segs.last().unwrap().end, 10 * MB + 3);
        for w in segs.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
    }

    #[test]
    fn steal_splits_largest_remaining() {
        let s = Scheduler::new(8 * MB, 1, MB);
        let a = s.next_job().unwrap();
        assert_eq!((a.pos, a.end), (0, 8 * MB));
        // 第二个 worker 劈开唯一的一段
        let b = s.next_job().unwrap();
        assert_eq!((b.pos, b.end), (4 * MB, 8 * MB));
        assert_eq!(s.end_of(a.idx), 4 * MB);
        // 段太小就不再劈
        let small = Scheduler::new(MB, 1, MB);
        small.next_job().unwrap();
        assert!(small.next_job().is_none());
    }

    #[test]
    fn claim_respects_shrunk_end() {
        let s = Scheduler::new(8 * MB, 1, MB);
        let a = s.next_job().unwrap();
        s.commit(a.idx, s.claim(a.idx, 100));
        let _b = s.next_job().unwrap();
        let end = s.end_of(a.idx);
        // 推进到快结束
        let mut pos = s.pos_of(a.idx);
        while end - pos > 10 {
            let n = s.claim(a.idx, (end - pos - 10).min(MAX_CHUNK as u64) as usize);
            s.commit(a.idx, n);
            pos += n as u64;
        }
        assert_eq!(s.claim(a.idx, 1000), 10);
        s.commit(a.idx, 10);
        assert_eq!(s.claim(a.idx, 1000), 0);
    }

    #[test]
    fn release_lets_others_pick_up() {
        let s = Scheduler::new(2 * MB, 2, MB);
        let a = s.next_job().unwrap();
        let _b = s.next_job().unwrap();
        s.release(a.idx);
        assert_eq!(s.next_job().unwrap().idx, a.idx);
    }
}
