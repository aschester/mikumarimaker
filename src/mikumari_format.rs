#![allow(unused)]
//! Contains the formatting stuff for mikumari data

pub const MIKUMARI_FRAME_ITEM_TYPE: u32=51;
use std::io::Read;
use std::io;

// Data type values:

pub const TDC_LEADING_DATA : u8 = 0b001011;
pub const TDC_TRAILING_DATA: u8 = 0b001101;
pub const INPUT_THROTTLE_T1_START : u8 = 0b0011001;
pub const INPUT_THROTTLE_T1_END : u8 = 0b010001;
pub const INPUT_THROTTLE_T2_START : u8 = 0b010010;
pub const INPUT_THROTTLE_T2_END : u8   = 0b010010;
pub const DELIMETER1 : u8 = 0b011100;
pub const DELIMETER2 : u8 = 0b011110;

/// A heartbeat delimieter 1 and its data.
pub struct Delimeter1 {
    delimeter : u64
}

/// A heartbeat delimieter 2 and its data.
pub struct Delimeter2 {
    delimeter : u64
}

/// Sample does not show the throttles so skip to the chase: Not sure how 
/// software tells the difference between high and low resolution as I don't 
/// see separate data types for them. The HRTDC carries time-over-threshold in 
/// the leading-edge word itself (issue #11), so a hit is reported once as a 
/// leading edge with its TOT.
pub struct HRTDCLeading {
    leading : u64
}
pub struct HRTDCTrailing {
    trailing : u64
}

pub struct LRTDCLeading {
    leading : u64
}
pub struct LRTDCTrailing {
    trailing : u64
}

// @todo: range check the inputs as they're not full sized.
impl Delimeter1 {
    pub fn new(time_offset : u16, frame_number: u32) -> Delimeter1 {
        let mut value : u64 = 0;
        value |= (DELIMETER1 as u64) << 58;
        value |= (time_offset as u64) << 24;
        value |= frame_number as u64;

        Delimeter1 {
            delimeter : value
        }
    }
    pub fn fromu64(data: u64) -> Delimeter1 {
        Delimeter1 {
            delimeter : data
        }
    }
    pub fn get(&self) -> u64 {
        self.delimeter
    }
    pub fn frame(&self) -> u64 {
        self.delimeter & 0xffffff
    }
    pub fn time_offset(&self) -> u64 {
        (self.delimeter >> 24) & 0xffff
    }
}

impl Delimeter2 {
    pub fn new(data_size: u32)-> Delimeter2 {
        let mut value = 0u64;
        value |= (DELIMETER2 as u64) << 58;
        let s = data_size as u64;
        value |= (s << 20) | s;

        Delimeter2 {
            delimeter : value
        }
    }
    pub fn fromu64(data: u64) -> Delimeter2 {
        Delimeter2 {
            delimeter: data
        }
    }
    pub fn get (&self) -> u64 {
        self.delimeter
    }    
    pub fn datasize(&self) -> u64 {
        self.delimeter & 0xfffff
    }  
}

impl HRTDCLeading {
    pub fn new(chan : u8, tot : u32, time : u32) -> HRTDCLeading {
        let mut value = (TDC_LEADING_DATA as u64) << 58;
        value |= (chan as u64) << 51;
        value |= (tot as u64)     << 29;
        value |= time as u64;

        HRTDCLeading {
            leading : value
        }
    }
    pub fn fromu64(data : u64)-> HRTDCLeading {
        HRTDCLeading  { leading : data}
    }
    pub fn channel(&self) -> u8 {        
        ((self.leading  >> 51) & 0x7f) as u8
    }
    pub fn tot(&self) -> u32 {
        ((self.leading >> 29) & 0x3fffff) as u32
    }
    pub fn time(&self) -> u32 {
        (self.leading & 0x1FFFFFFF) as u32
    }
    pub fn get(&self) -> u64 {
        self.leading
    }
}

// In fact, other than the data type field, this is just like the leading edge
// so we do some dirty stuff.
impl HRTDCTrailing {
    pub fn new(chan : u8, tot : u32, time : u32) -> HRTDCTrailing {
        let leading = HRTDCLeading::new(chan, tot, time);
        // Mask off the data type and replace it with the trailing data type:
        let mut data = leading.leading;
        data &= !((TDC_LEADING_DATA as u64) << 58);
        data |= (TDC_TRAILING_DATA as u64)<< 58;

        HRTDCTrailing {
            trailing : data
        }
    }
    pub fn fromu64(data : u64) -> HRTDCTrailing {
        HRTDCTrailing {
            trailing: data
        }
    }
    // Here's where the dirt is, we use the LE functions. The dirt we use in 
    // this method gets used in all others as well.
    pub fn channel(&self) -> u8 {
        let leading = HRTDCLeading { leading: self.trailing};        
        leading.channel()
    }
    pub fn tot(&self) -> u32 {
        let leading = HRTDCLeading { leading: self.trailing};
        leading.tot()
    }
    pub fn time(&self) -> u32 {
        let leading = HRTDCLeading { leading: self.trailing};
        leading.time()
    }
    pub fn get(&self) -> u64 {
        self.trailing
    }
}

/// This enum is data that can come from a Mikumari data source.
pub enum MikumariDatum {
    Heartbeat0(Delimeter1),
    Heartbeat1(Delimeter2),
    LeadingEdge(HRTDCLeading),
    TrailingEdge(HRTDCTrailing),
    Other(u64)
}

impl MikumariDatum {
    pub fn from_u64(datum : u64 ) -> MikumariDatum {
        // Width of the word is 64 bits, the type is in the top 6 bits:
        let dtype :u8 = (datum >> (64-6)) as u8; // Position the type.
        if dtype == TDC_LEADING_DATA {
            MikumariDatum::LeadingEdge(HRTDCLeading::fromu64(datum))
        } else if dtype == TDC_TRAILING_DATA {
            MikumariDatum::TrailingEdge(HRTDCTrailing::fromu64(datum))
        } else if dtype == DELIMETER1 {
            MikumariDatum::Heartbeat0(Delimeter1::fromu64(datum))
        } else if dtype== DELIMETER2 {
            MikumariDatum::Heartbeat1(Delimeter2::fromu64(datum))
        } else {
            MikumariDatum::Other(datum)
        }
    }
}

pub struct MikumariReader {
    source : Box<dyn Read>,
}

impl MikumariReader {
    // Read the next u64 for the data source:
    fn readu64(&mut self) -> io::Result<u64> {
        let mut buf : [u8;8] = [0;8];
        self.source.read_exact(&mut buf)?;

        Ok(u64::from_ne_bytes(buf))

    }
    pub fn new(src : Box<dyn Read>) -> MikumariReader  {
        MikumariReader {
            source : src
        }
    }
    pub fn read(&mut self) -> io::Result<MikumariDatum> {
        let datum = self.readu64()?;
        // Based on the format field, we return the right type of datum:
        Ok(MikumariDatum::from_u64(datum))
    }
} 

#[cfg(test)]
mod delim1test {
    use super::*;

    #[test]
    fn new_1() {
        let d = Delimeter1::new(0,0);
        assert_eq!(d.get(), 0x7000000000000000)
    }
    #[test]
    fn new_2() {
        let d = Delimeter1::new(0, 1234);
        assert_eq!(d.get() & 0xfffff, 1234u64);
    }
    #[test]
    fn new_3() {
        let d = Delimeter1::new(65535, 0);
        assert_eq!((d.get() >> 24) & 0xffff, 65535);
    }
}

#[cfg(test)]
mod delim2test {
    use super::*;

    #[test]
    fn new_1() {
        let d = Delimeter2::new(0);   // Just the id is set.

        assert_eq!(d.get(), 0x7800000000000000);
    }
    #[test]
    fn new_2() {
        let d = Delimeter2::new(12345);
        assert_eq!(d.get() & 0xfffff, 12345u64);
    }
    #[test]
    fn new_3() {
        let d = Delimeter2::new(12345); 
        assert_eq!((d.get() >> 20) & 0xfffff, 12345u64);
    }
}

#[cfg(test)] 
mod hrtdc {
    use super::*;
    #[test]
    // Leading edge tests:
        fn leading_new() {
        let leading = HRTDCLeading::new(10, 100, 12345);

        // See that the fields got properly set as well as the type:

        assert_eq!(leading.leading >>58, TDC_LEADING_DATA as u64);
        assert_eq!(leading.leading & 0x1fffffff, 12345);
        assert_eq!((leading.leading >> 29) & 0x3fffff, 100);
    }
    #[test]
        fn chan_1() {
        
        let leading = HRTDCLeading::new(10, 100, 12345);
        assert_eq!(leading.channel(), 10);
    }
    #[test]
        fn tot_1() {
        let leading = HRTDCLeading::new(10, 100, 12345);
        assert_eq!(leading.tot(), 100);
    }
    #[test]
        fn time_1() {
        let leading = HRTDCLeading::new(10, 100, 12345);
        assert_eq!(leading.time(), 12345);
    }

    // Trailing edge tests:

    #[test]
        fn trailing_new() {
        let trailing = HRTDCTrailing::new(10, 100, 12345);

        // See that the fields got properly set as well as the type:

        assert_eq!(trailing.trailing >>58, TDC_TRAILING_DATA as u64);
        assert_eq!(trailing.trailing & 0x1fffffff, 12345);
        assert_eq!((trailing.trailing >> 29) & 0x3fffff, 100);
    }
    #[test]
        fn chan_2() {
        
        let trailing = HRTDCTrailing::new(10, 100, 12345);
        assert_eq!(trailing.channel(), 10);
    }
    #[test]
        fn tot_2() {
        let trailing = HRTDCTrailing::new(10, 100, 12345);
        assert_eq!(trailing.tot(), 100);
    }
    #[test]
        fn time_2() {
        let trailing = HRTDCTrailing::new(10, 100, 12345);
        assert_eq!(trailing.time(), 12345);
    }
}