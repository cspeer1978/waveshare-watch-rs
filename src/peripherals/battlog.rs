// Battery log: a ring buffer of small records in the watch's flash, so battery
// history survives the battery dying and re-flashing.
//
// Where: 64 KB (16 sectors) at the 12 MB mark of the 32 MB flash (below 16 MB,
// so plain 3-byte flash addressing reaches it). The firmware image is ~0.9 MB
// at the start of flash, and flashing the merged .bin only writes that much,
// so this area is never touched by updates.
//
// Two 16-byte record kinds (little endian), told apart by the DIAG flag:
//   normal: seq u32 | minute u32 | mv u16 | screen_s u16 | pct u8 | flags u8 | radio4 u8 | crc u8
//   diag:   seq u32 | minute u32 | sleep_s u16 | awake_s u16 | timer_wakes u8 | flags u8 | gpio_wakes u8 | crc u8
//   minute  = minutes since 2000-01-01 (watch local time, from the RTC)
//   flags   = 0xA0 magic in the high nibble | CHARGING | USB | BOOT | DIAG
//   radio4  = Wi-Fi-on time in 4-second units since the previous record
//   sleep_s = time in light sleep since the previous record
//   awake_s = time awake with the screen off since the previous record
//   *_wakes = light-sleep wake-ups by the timer / by touch or the BOOT button
//   crc     = XOR of bytes 0..15 ^ 0x5A
// A diag record is written right after its normal record (seq + 1) and is
// merged into that entry when the log is read. Older logs without diag
// records still read fine.
// The magic + checksum mean leftover data from other firmware can't be
// mistaken for records. Writes use raw NOR-flash programming: a sector is
// erased only when the ring wraps onto it (about once every 21 hours).
// 4096 records at two per ~10 minutes = about 2 weeks of history.

use alloc::vec::Vec;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_storage::FlashStorage;

pub const LOG_BASE: u32 = 0x00C0_0000;
pub const SECTOR: u32 = 4096;
pub const LOG_SECTORS: u32 = 16;
pub const LOG_SIZE: u32 = SECTOR * LOG_SECTORS;
pub const REC: u32 = 16;

pub const F_CHARGING: u8 = 0x01;
pub const F_USB: u8 = 0x02;
pub const F_BOOT: u8 = 0x04;
pub const F_DIAG: u8 = 0x08;
const MAGIC: u8 = 0xA0;

/// 4-byte aligned buffers, so the flash driver uses them directly
/// instead of copying through a 4 KB temporary buffer on the stack.
#[repr(C, align(4))]
struct Aligned([u8; 16]);
#[repr(C, align(4))]
struct AlignedBuf([u8; 256]);

#[derive(Clone, Copy, Debug, Default)]
pub struct Entry {
    pub seq: u32,
    pub minute: u32,
    pub mv: u16,
    pub screen_s: u16,
    pub pct: u8,
    pub flags: u8,     // low nibble only (magic stripped); never has F_DIAG
    pub radio_s: u16,  // seconds (stored in 4 s units)
    // Sleep diagnostics (from the matching diag record; zero if none)
    pub has_diag: bool,
    pub sleep_s: u16,
    pub awake_s: u16,
    pub timer_wakes: u8,
    pub gpio_wakes: u8,
}

fn crc(b: &[u8]) -> u8 { b[..15].iter().fold(0x5Au8, |a, &x| a ^ x) }

impl Entry {
    fn encode(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&self.seq.to_le_bytes());
        b[4..8].copy_from_slice(&self.minute.to_le_bytes());
        b[8..10].copy_from_slice(&self.mv.to_le_bytes());
        b[10..12].copy_from_slice(&self.screen_s.to_le_bytes());
        b[12] = self.pct;
        b[13] = MAGIC | (self.flags & 0x07);
        b[14] = (self.radio_s / 4).min(255) as u8;
        b[15] = crc(&b);
        b
    }

    fn encode_diag(&self, seq: u32) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&seq.to_le_bytes());
        b[4..8].copy_from_slice(&self.minute.to_le_bytes());
        b[8..10].copy_from_slice(&self.sleep_s.to_le_bytes());
        b[10..12].copy_from_slice(&self.awake_s.to_le_bytes());
        b[12] = self.timer_wakes;
        b[13] = MAGIC | F_DIAG | (self.flags & 0x07);
        b[14] = self.gpio_wakes;
        b[15] = crc(&b);
        b
    }
}

/// A decoded raw record.
enum Rec { Normal(Entry), Diag { seq: u32, sleep_s: u16, awake_s: u16, timer_wakes: u8, gpio_wakes: u8 } }

impl Rec {
    fn seq(&self) -> u32 { match self { Rec::Normal(e) => e.seq, Rec::Diag { seq, .. } => *seq } }

    fn decode(b: &[u8]) -> Option<Self> {
        if b[13] & 0xF0 != MAGIC { return None; }
        if crc(b) != b[15] { return None; }
        let seq = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if seq == u32::MAX { return None; }
        if b[13] & F_DIAG != 0 {
            return Some(Rec::Diag {
                seq,
                sleep_s: u16::from_le_bytes([b[8], b[9]]),
                awake_s: u16::from_le_bytes([b[10], b[11]]),
                timer_wakes: b[12],
                gpio_wakes: b[14],
            });
        }
        Some(Rec::Normal(Entry {
            seq,
            minute: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            mv: u16::from_le_bytes([b[8], b[9]]),
            screen_s: u16::from_le_bytes([b[10], b[11]]),
            pct: b[12],
            flags: b[13] & 0x07,
            radio_s: b[14] as u16 * 4,
            ..Default::default()
        }))
    }
}

pub struct BattLog {
    flash: FlashStorage<'static>,
    head: u32,       // offset (within the log area) of the next slot to write
    next_seq: u32,
    pub ok: bool,    // false if flash access failed (logging then does nothing)
}

impl BattLog {
    /// Scan the log area to find where to continue writing.
    pub fn new(flash: FlashStorage<'static>) -> Self {
        let mut log = Self { flash, head: 0, next_seq: 0, ok: true };
        let mut best: Option<(u32, u32)> = None; // (seq, offset)
        let mut buf = AlignedBuf([0u8; 256]);
        let mut off = 0;
        while off < LOG_SIZE {
            if ReadNorFlash::read(&mut log.flash, LOG_BASE + off, &mut buf.0).is_err() { log.ok = false; return log; }
            for k in 0..(buf.0.len() as u32 / REC) {
                let s = (k * REC) as usize;
                if let Some(r) = Rec::decode(&buf.0[s..s + REC as usize]) {
                    let seq = r.seq();
                    if best.map_or(true, |(bs, _)| seq > bs) { best = Some((seq, off + k * REC)); }
                }
            }
            off += buf.0.len() as u32;
        }
        if let Some((seq, o)) = best {
            log.next_seq = seq.wrapping_add(1);
            log.head = (o + REC) % LOG_SIZE;
        }
        log
    }

    fn write_rec(&mut self, rec: [u8; 16]) -> bool {
        let addr = LOG_BASE + self.head;
        // Make sure the slot is blank; if not (start of a reused sector, or
        // leftover foreign data), erase its sector first.
        let mut cur = Aligned([0u8; 16]);
        if ReadNorFlash::read(&mut self.flash, addr, &mut cur.0).is_err() { return false; }
        if cur.0.iter().any(|&b| b != 0xFF) {
            let sector_start = addr - (addr - LOG_BASE) % SECTOR;
            if NorFlash::erase(&mut self.flash, sector_start, sector_start + SECTOR).is_err() { return false; }
        }
        let rec = Aligned(rec);
        if NorFlash::write(&mut self.flash, addr, &rec.0).is_err() { return false; }
        self.head = (self.head + REC) % LOG_SIZE;
        self.next_seq = self.next_seq.wrapping_add(1);
        true
    }

    /// Append one entry (plus its diag record when `e.has_diag`).
    /// Returns false if the flash write failed.
    pub fn append(&mut self, mut e: Entry) -> bool {
        if !self.ok { return false; }
        e.seq = self.next_seq;
        if !self.write_rec(e.encode()) { return false; }
        if e.has_diag {
            let seq = self.next_seq;
            if !self.write_rec(e.encode_diag(seq)) { return false; }
        }
        true
    }

    /// All valid entries (diag data merged in), oldest first.
    pub fn read_all(&mut self) -> Vec<Entry> {
        let mut recs: Vec<Rec> = Vec::new();
        if !self.ok { return Vec::new(); }
        let mut buf = AlignedBuf([0u8; 256]);
        let mut off = 0;
        while off < LOG_SIZE {
            if ReadNorFlash::read(&mut self.flash, LOG_BASE + off, &mut buf.0).is_err() { break; }
            for k in 0..(buf.0.len() / REC as usize) {
                let s = k * REC as usize;
                if let Some(r) = Rec::decode(&buf.0[s..s + REC as usize]) { recs.push(r); }
            }
            off += buf.0.len() as u32;
        }
        recs.sort_unstable_by_key(|r| r.seq());
        let mut out: Vec<Entry> = Vec::with_capacity(recs.len());
        for r in recs {
            match r {
                Rec::Normal(e) => out.push(e),
                Rec::Diag { seq, sleep_s, awake_s, timer_wakes, gpio_wakes } => {
                    if let Some(last) = out.last_mut() {
                        if last.seq.wrapping_add(1) == seq {
                            last.has_diag = true;
                            last.sleep_s = sleep_s;
                            last.awake_s = awake_s;
                            last.timer_wakes = timer_wakes;
                            last.gpio_wakes = gpio_wakes;
                        }
                    }
                }
            }
        }
        out
    }
}

/// Minutes since 2000-01-01 00:00 for an RTC date/time (year 0..99 = 20YY).
pub fn rtc_minute(year: u8, month: u8, day: u8, h: u8, m: u8) -> u32 {
    let days = crate::apps::sunmoon::days_from_civil(2000 + year as i32, month.max(1) as u32, day.max(1) as u32)
        - crate::apps::sunmoon::days_from_civil(2000, 1, 1);
    (days.max(0) as u32) * 1440 + h as u32 * 60 + m as u32
}

/// Battery % from voltage, measured on this watch's 400 mAh cell
/// (Oct 20 2026 drain test, ~33 mA average load: the voltage fell almost
/// linearly with time, so time fraction = charge fraction).
/// The power chip's own % uses a generic profile that runs far too early.
const OCV: [(u16, u8); 17] = [
    (4120, 100), (4058, 98), (3998, 92), (3942, 86), (3875, 79), (3825, 72),
    (3782, 66), (3753, 60), (3715, 54), (3680, 47), (3651, 40), (3622, 34),
    (3594, 28), (3560, 22), (3519, 16), (3480, 10), (3249, 3),
];

pub fn volt_pct(mv: u16) -> u8 {
    if mv >= OCV[0].0 { return 100; }
    for w in OCV.windows(2) {
        let (v1, p1) = w[0];
        let (v0, p0) = w[1];
        if mv >= v0 {
            let f = (mv - v0) as u32 * 100 / (v1 - v0) as u32;
            return (p0 as u32 + (p1 - p0) as u32 * f / 100) as u8;
        }
    }
    if mv <= 3054 { 0 } else { (3 * (mv - 3054) as u32 / (3249 - 3054) as u32) as u8 }
}

/// Battery capacity used for mAh / runtime figures.
pub const CAPACITY_MAH: u32 = 400;
