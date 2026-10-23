//! Host stand-in for esp-storage: 32 MB of RAM "flash" with NOR semantics
//! (write can only clear bits; erase sets a sector to 0xFF).
use embedded_storage::nor_flash::{ErrorType, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash};
use std::marker::PhantomData;
#[derive(Debug)] pub struct E;
impl NorFlashError for E { fn kind(&self) -> NorFlashErrorKind { NorFlashErrorKind::Other } }
pub struct FlashStorage<'d> { pub mem: Vec<u8>, pub erases: u32, _p: PhantomData<&'d ()> }
impl<'d> FlashStorage<'d> { pub fn new_host() -> Self { Self { mem: vec![0xFF; 32 << 20], erases: 0, _p: PhantomData } } }
impl<'d> ErrorType for FlashStorage<'d> { type Error = E; }
impl<'d> ReadNorFlash for FlashStorage<'d> {
    const READ_SIZE: usize = 4;
    fn read(&mut self, off: u32, b: &mut [u8]) -> Result<(), E> { let o = off as usize; b.copy_from_slice(&self.mem[o..o + b.len()]); Ok(()) }
    fn capacity(&self) -> usize { self.mem.len() }
}
impl<'d> NorFlash for FlashStorage<'d> {
    const WRITE_SIZE: usize = 4; const ERASE_SIZE: usize = 4096;
    fn erase(&mut self, a: u32, b: u32) -> Result<(), E> { assert!(a % 4096 == 0 && b % 4096 == 0); for x in &mut self.mem[a as usize..b as usize] { *x = 0xFF; } self.erases += 1; Ok(()) }
    fn write(&mut self, off: u32, b: &[u8]) -> Result<(), E> { for (i, &v) in b.iter().enumerate() { let c = &mut self.mem[off as usize + i]; assert!(*c == 0xFF, "write over unerased byte"); *c &= v; } Ok(()) }
}
