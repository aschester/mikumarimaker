//!
//! This crate provides the code needed to glom hits into physics ring  items.
//! 
//! 
//! 
use frib_datasource::DataSink;
use rust_ringitem_format::{RingItem, PHYSICS_EVENT};
/// The Glom struct and its implementation are what 
/// do the work.
///  Note that we can add hits and frame boundaries to the
///  event being accumulated.
///  Events are timestamped with the timestamp of the first hit.
/// 
///   hits are stored, internally, as a 16 bit channel/edge number
///  and edge bit and a 64 bit time relative to the start of run.
///  The frame boundary marker is stored as an absolute  frame number with
///  all bits set in the channel/edge word.
///  If the first hit is a frame boundary, it does not start a coincidence interval.
pub struct Glom {
    sink : Box<dyn DataSink>,       // Anything writable.
    sid  : u32,                     // Source id.
    dt   : u64,                     // coincidence interval.
    t0   : Option<u64>,             // when some, the start time of the glom.
    hits : Vec<(u16, u64, u32)>,    // Hits accumulated so far. Issue #11 add TOT.
}

impl Glom {
    // Start a new hit:
    fn new_event(&mut self, chan : u16, time: u64, tot: u32) {
        self.t0 = Some(time);
        self.hits.push((chan, time, tot));
    }
    /// Flush the frame as a ring item. 
    /// Note that this is a no-op if t0 is None (e.g. maybe at end of run?).
    /// t0 will be set to None and hits cleared.
    /// Note that if hits are only frame boundaries, this can lead, at the end run,
    /// dropping them on the floor...why do this? Because we're not sure how to timestamp
    /// frame boundaries.
    pub fn flush(&mut self) {
        if let Some(stamp) = self.t0 {
            let mut item = RingItem::new_with_body_header(
                PHYSICS_EVENT,
                stamp, self.sid, 0
            );
            // Fill the body with hits:
            for (ch, t, tot) in &self.hits {
                item.add(*ch);
                item.add(*t);
                item.add(*tot);              // Issue #11
            }
            self.sink.write(&item).expect("Unable to write a ring item");
            self.sink.flush();
            self.hits.clear();
            self.t0 = None;
        }
        
    }

    /// Create a glommer, the 
    /// dt and sink are required we set the t0 as none hits as empty.
    ///
    /// ### Parameters:
    /// *   sink - a data sink. The Glom gains ownership.
    /// *   sid  - Source id to put in the ring item body headers.
    /// *   dt   - ticks in coincidence interval.
    /// ### Returns:
    /// a Glom struct.
    /// 
    pub fn new(sink : Box<dyn DataSink>, sid : u32, dt : u64) -> Glom {
        Glom {
            sink : sink,
            sid  : sid,
            dt   : dt,
            t0   : None,            // Not making one.
            hits : Vec::new()
        }
    }
    /// Alter the sid...
    pub fn set_sid(&mut self, sid:  u32) {
        self.sid = sid;
    }
    /// Sometimes we need to just output a ring item.
    /// Since we own the data sink, this allows that:
    ///
    /// ### Parameters:
    /// * item - references a ring itemt to write unaltered.
    /// 
    /// ### Notes:
    /// *  Panics if unable to write.
    /// *  This has no effect on the t0, hits.  At the end of the run, presumably
    ///    one does a flush to write what's there first and then passes the end run item.
    /// 
    pub fn write_item(&mut self, item: &RingItem) {
        self.sink.write(&item).expect("Failed to pass through a ring item");
        self.sink.flush();
    }
    ///
    /// Add a frame boundary to the hits.  This does not
    /// have any effect on the t0 value.
    /// 
    /// ### Parameters:
    /// * fno - absolute frame number.
    /// 
    pub fn add_frame_boundary(&mut self,fno : u64) {
        self.hits.push((0xffff, fno, 0xffffffff));   // issue #11
    }
    ///
    /// Add a hit.  We construct the channel number word from the channel number
    /// and leading flag.  There are two cases to handle 
    /// 1.  t0 is None. In that case, we are a first hit and set t0 to Some(time). 
    /// and add the channel/time to the hits vector.
    /// 2. t0 is Some, in which case, if we are in the glom interval we just add our hit,
    /// otherwise, flush and start a new event.
    /// 
    /// ###  Parameters
    /// * leading - true if this is a leading edge hit.
    /// * channel - The channel number.
    /// * time    - The absolute time of the hit.
    /// * tot     - Time over threshold.
    /// 
    pub fn add_hit(&mut self, leading : bool, channel : u8, time : u64, tot : u32) {
        // Construct the u16 channel/edge tag.

        let chanword : u16 = if leading {
            channel as u16
        } else {
            channel as u16 | 0x8000
        };
        match self.t0 {
            None => self.new_event(chanword, time, tot),
            Some(t0) => {
                if time - t0 <= self.dt {
                    self.hits.push((chanword, time, tot));
                } else {
                    self.flush();
                    self.new_event(chanword, time, tot);
                }
            }
        }
    }
}

#[cfg(test)]
mod glom_tests {
    use super::*;

    // Here's a struct that implements a data sink for our tests. It just copies the ring item.

    struct TestSink {
        item : Option<RingItem>
    }
    impl DataSink for TestSink {
        fn open(&mut self, _uri: &str) -> Result<(), String> {Ok(())}
        fn write(&mut self, item : &RingItem) ->Result<(), String> {
            // sure wish I'd implemented ring ittem clone but I didn't so:
            let mut body_offset = 0;
            let mut new_item = if item.has_body_header() {
                let bh = item.get_bodyheader().unwrap();
                body_offset = size_of::<u32>()*2 + size_of::<u64>();      // Payload has the body header.
                RingItem::new_with_body_header(item.type_id(), bh.timestamp, bh.source_id, bh.barrier_type)
            } else {
                RingItem::new(item.type_id())
            };
            // put the body in:
            
            let p = item.payload();
            for i in body_offset..p.len() {
                new_item.add(p[i]);
            }
            self.item  = Some(new_item);

            Ok(())
        }
        fn close(&mut self) {}
        fn flush(&mut self) {}
    }

    #[test]
    fn new_1() {
        let sink = TestSink {item: None};
        let glom = Glom::new(Box::new(sink), 1, 100);
        assert_eq!(glom.sid, 1);
        assert_eq!(glom.dt, 100);
        assert!(glom.t0.is_none());
        assert!(glom.hits.is_empty());
    }
    #[test]
    fn set_sid_1() {
        // Can change the source id:

        let sink = TestSink {item: None};
        let mut glom = Glom::new(Box::new(sink), 1, 100);
        glom.set_sid(2);
        assert_eq!(glom.sid, 2);
    }
    #[test]
    fn write_item_1() {
        // Can do pass through on an item.

        let sink = Box::new(TestSink {item: None});
        let p    = Box::into_raw(sink);
        let rsink = unsafe {&*p};
        let x = unsafe { Box::from_raw(p)};
        
        let mut glom = Glom::new(x, 1, 100);
        let mut item = RingItem::new_with_body_header(
            PHYSICS_EVENT,
            100, 2, 0
        );
        for i in 0..10 {
            let b : u8 = i;
            item.add(b);
        }
        glom.write_item(&item);
        
        assert!(rsink.item.is_some());
        let item = rsink.item.as_ref().unwrap();
        assert_eq!(item.type_id(), PHYSICS_EVENT);
        assert!(item.has_body_header());
        let bh = item.get_bodyheader().unwrap();
        assert_eq!(bh.timestamp, 100);
        assert_eq!(bh.source_id, 2);
        assert_eq!(bh.barrier_type, 0);

        let bytes = item.payload();
        let mut v = 0;
        for i in 2*size_of::<u32>()+size_of::<u64>()..bytes.len() {
            assert_eq!(bytes[i], v);
            v += 1;
        }
    
    }
    #[test]
    fn add_frame_1() {
        // Add a frame boundary.

        let sink = Box::new(TestSink {item: None});
        let p    = Box::into_raw(sink);
        let rsink = unsafe {&*p};
        let x = unsafe { Box::from_raw(p)};
        
        let mut glom = Glom::new(x, 1, 100);

        glom.add_frame_boundary(123);

        // Should add an pseudo hit but not write:

        assert_eq!(glom.hits.len(), 1);
        assert_eq!(glom.hits[0], (0xffffu16, 123u64, 0xffffffffu32));
        assert!(rsink.item.is_none());
    }   
    #[test]
    fn add_hit_1() {
        let sink = Box::new(TestSink {item: None});
        let p    = Box::into_raw(sink);
        let rsink = unsafe {&*p};
        let x = unsafe { Box::from_raw(p)};
        
        let mut glom = Glom::new(x, 1, 100);
        glom.add_hit(true, 1, 0, 666);    // The hit.
        assert_eq!(glom.hits.len(), 1);
        assert_eq!(glom.hits[0], (1u16, 0u64, 666u32));
        assert!(rsink.item.is_none());

    } 
    #[test]
    fn add_hit_2() {
        let sink = Box::new(TestSink {item: None});
        let p    = Box::into_raw(sink);
        let rsink = unsafe {&*p};
        let x = unsafe { Box::from_raw(p)};
        
        let mut glom = Glom::new(x, 1, 100);
        glom.add_hit(true, 1, 0, 666);    // The hit.
        glom.add_frame_boundary(123);
        assert_eq!(glom.hits.len(), 2);
        assert_eq!(glom.hits[0], (1u16, 0u64, 666u32));
        assert_eq!(glom.hits[1], (0xffffu16, 123u64, 0xffffffffu32));
        assert!(rsink.item.is_none());
    }
    #[test]
    fn add_hit_3() {
        // Two hits inside dt don't write

        let sink = Box::new(TestSink {item: None});
        let p    = Box::into_raw(sink);
        let rsink = unsafe {&*p};
        let x = unsafe { Box::from_raw(p)};
        
        let mut glom = Glom::new(x, 1, 100);
        glom.add_hit(true, 1, 0, 666);    // The first hit.
        glom.add_hit(true, 0, 50, 666);   // dt is 100.

        assert_eq!(glom.hits.len(), 2);
        assert_eq!(glom.hits[0], (1u16, 0u64, 666u32));
        assert_eq!(glom.hits[1], (0u16, 50u64, 666u32));
        assert!(rsink.item.is_none());
    }
    #[test]
    fn add_hit_4() {
        // two hits outside dt writes the first.
        let sink = Box::new(TestSink {item: None});
        let p    = Box::into_raw(sink);
        let rsink = unsafe {&*p};
        let x = unsafe { Box::from_raw(p)};
        
        let mut glom = Glom::new(x, 1, 100);
        glom.add_hit(true, 1, 50, 666);    // The first hit.
        glom.add_hit(true, 0, 151, 666);   // dt is 100.

        assert_eq!(glom.hits.len(), 1);    // Second hit still retained.
        assert_eq!(glom.hits[0], (0u16, 151u64, 666u32));   // this is hit 0.

        // Should have written:

        assert!(rsink.item.is_some());
        let item = rsink.item.as_ref().unwrap();
        assert_eq!(item.type_id(), PHYSICS_EVENT);
        assert!(item.has_body_header());        // THere is a body header and...
        let bh = item.get_bodyheader().unwrap();
        assert_eq!(bh.timestamp, 50);           // body header has 1'st item timestamp.
        assert_eq!(bh.source_id, 1);
        assert_eq!(bh.barrier_type, 0); 

        let body_offset = size_of::<u64>() + 2*size_of::<u32>();
        let payload = &item.payload()[body_offset..];

        // Body has one hit:

        assert_eq!(payload.len(), size_of::<u16>() + size_of::<u64>() + size_of::<u32>());

        let chan  = u16::from_le_bytes(payload[0..2].try_into().unwrap());
        assert_eq!(chan, 1);

        let ts = u64::from_le_bytes(payload[2..10].try_into().unwrap());  
        assert_eq!(ts, 50);

        let tot = u32::from_le_bytes(payload[10..14].try_into().unwrap());
        assert_eq!(tot, 666);
    }
    #[test]
    fn add_hit_5() {
        // a hit, frame then a hit outside dt writes
        // the hit and frame boundary.
        let sink = Box::new(TestSink {item: None});
        let p    = Box::into_raw(sink);
        let rsink = unsafe {&*p};
        let x = unsafe { Box::from_raw(p)};
        
        let mut glom = Glom::new(x, 1, 100);
        glom.add_hit(true, 1, 50, 666);    // The first hit.
        glom.add_frame_boundary(10);
        glom.add_hit(true, 0, 151, 666);   // dt is 100.

        assert_eq!(glom.hits.len(), 1);    // Second hit still retained.
        assert_eq!(glom.hits[0], (0u16, 151u64, 666u32));   // this is hit 0.

        // Should have written:

        assert!(rsink.item.is_some());
        let item = rsink.item.as_ref().unwrap();
        assert_eq!(item.type_id(), PHYSICS_EVENT);
        assert!(item.has_body_header());        // THere is a body header and...
        let bh = item.get_bodyheader().unwrap();
        assert_eq!(bh.timestamp, 50);           // body header has 1'st item timestamp.
        assert_eq!(bh.source_id, 1);
        assert_eq!(bh.barrier_type, 0); 

        let body_offset = size_of::<u64>() + 2*size_of::<u32>();
        let payload = &item.payload()[body_offset..];

        // size of the payload is 2 hits:

        let hit_size = size_of::<u16>() + size_of::<u64>() + size_of::<u32>();
        assert_eq!(payload.len(), hit_size*2);

        
        let chan  = u16::from_le_bytes(payload[0..2].try_into().unwrap());
        assert_eq!(chan, 1);

        let ts = u64::from_le_bytes(payload[2..10].try_into().unwrap());  
        assert_eq!(ts, 50);

        let tot = u32::from_le_bytes(payload[10..14].try_into().unwrap());
        assert_eq!(tot, 666);

        let chan  = u16::from_le_bytes(payload[hit_size..hit_size+2].try_into().unwrap());
        assert_eq!(chan, 0xffff);

        let ts = u64::from_le_bytes(payload[hit_size+2..hit_size+10].try_into().unwrap());  
        assert_eq!(ts, 10);

        let tot = u32::from_le_bytes(payload[hit_size+10..hit_size+14].try_into().unwrap());
        assert_eq!(tot, 0xffffffff);
    }
    #[test]
    fn add_hit_6() {
        // two hits inside dt followed by one out writes the first two.
        
        let sink = Box::new(TestSink {item: None});
        let p    = Box::into_raw(sink);
        let rsink = unsafe {&*p};
        let x = unsafe { Box::from_raw(p)};
        
        let mut glom = Glom::new(x, 1, 100);
        glom.add_hit(true, 1, 50, 666);    // The first hit.
        glom.add_hit(true, 0, 75, 666);    // second hit in time window.
        glom.add_hit(true, 1, 151, 666);   // outside of window.

        assert_eq!(glom.hits.len(), 1);    // Second hit still retained.
        assert_eq!(glom.hits[0], (1u16, 151u64, 666u32));   // this is hit 0.

        // Should have written:

        assert!(rsink.item.is_some());
        let item = rsink.item.as_ref().unwrap();
        assert_eq!(item.type_id(), PHYSICS_EVENT);
        assert!(item.has_body_header());        // THere is a body header and...
        let bh = item.get_bodyheader().unwrap();
        assert_eq!(bh.timestamp, 50);           // body header has 1'st item timestamp.
        assert_eq!(bh.source_id, 1);
        assert_eq!(bh.barrier_type, 0); 

        let body_offset = size_of::<u64>() + 2*size_of::<u32>();
        let payload = &item.payload()[body_offset..];

        // size of the payload is 2 hits:

        let hit_size = size_of::<u16>() + size_of::<u64>() + size_of::<u32>();
        assert_eq!(payload.len(), hit_size*2);

        let chan  = u16::from_le_bytes(payload[0..2].try_into().unwrap());
        assert_eq!(chan, 1);

        let ts = u64::from_le_bytes(payload[2..10].try_into().unwrap());  
        assert_eq!(ts, 50);

        let tot = u32::from_le_bytes(payload[10..14].try_into().unwrap());
        assert_eq!(tot, 666);

        let chan  = u16::from_le_bytes(payload[hit_size..hit_size+2].try_into().unwrap());
        assert_eq!(chan, 0);

        let ts = u64::from_le_bytes(payload[hit_size+2..hit_size+10].try_into().unwrap());  
        assert_eq!(ts, 75);

        let tot = u32::from_le_bytes(payload[hit_size+10..hit_size+14].try_into().unwrap());
        assert_eq!(tot, 666);

    }
}
/// Merges hits into a fully time ordered stream.
/// The output of this can be inserted into a Glom
/// to build events.
///   The idea is that we feed a frame at a time into this and
/// pull the hits out, feeding those to a Glom.
pub struct Orderer {
    hits : Vec<(bool, u16, u64, u32)>,  // Soup of hits.
}
impl Orderer {
    /// Create a new orderer.
    pub fn new() -> Orderer {
        Orderer {
            hits: Vec::new()
        }
    }
    /// Add a hit to be orderered:
    /// 
    /// ### Parameters
    /// *  rising - true if this hit is a rising edge.
    /// *  chan   - channel number of the hit.
    /// *  time   - Time at which the hit happened (sort key).
    /// *  tot    - Time over threshold.
    pub fn add_hit(&mut self, rising : bool, chan : u16, time : u64, tot : u32) {
        self.hits.push((rising, chan, time, tot));
    }
    /// Return  the ordered hits and clear the accumulated array:
    /// 
    /// ### Returns:
    /// Vec<(bool, u16, u64, u32)> - rising flag, channel, time.
    /// 
    /// ### Notes:
    /// *   The hits are ordered using sort_unstable_by_key.
    /// *   The hits array is cleared after it is cloned for return:
    pub fn order(&mut self) -> Vec<(bool, u16, u64, u32)> {
        self.hits.sort_unstable_by_key(|k| k.2);
        let result = self.hits.clone();
        self.hits.clear();
        result
    }
}
#[cfg(test)]
mod orderer_tests {
    use super::*;
    use rand::{RngExt};
    #[test]
    fn construct_1() {
        let o = Orderer::new();
        assert!(o.hits.is_empty());
    }
    #[test]
    fn order_1() {
        // No hits gives an empty orderer:

        let mut o = Orderer::new();
        let order = o.order();
        assert!(order.is_empty());
    }
    #[test]
    fn hit_1() {
        // I can add a hit and it's there.

        let mut o = Orderer::new();
        o.add_hit(true, 1, 12345, 666);
        assert_eq!(o.hits.len(), 1);      // there is a hit.

        assert_eq!(o.hits[0], (true, 1, 12345, 666));
    }
    #[test]
    fn order_2() {
        // If I add one hit and order it it'll come out unscathed.

        let mut o = Orderer::new();
        o.add_hit(true, 1, 12345, 666);
        let ordered = o.order();
        assert_eq!(ordered.len(), 1);
        assert_eq!(ordered[0], (true, 1, 12345, 666));
    }
    #[test]
    fn order_3() {
        // Adding some ordered hits they come out with the same order
        // they were put in.

        let mut o = Orderer::new();
        for i  in 0..10 {
            o.add_hit(true, i as u16 % 2 , i as u64, 666);
        }
        let ordered = o.order();
        assert_eq!(ordered.len(), 10);

        for i in 0..10 {
            assert_eq!(ordered[i], (true, i as u16 % 2, i as u64, 666));
        }
    }
    #[test]
    fn order_4() {
        // Fully backwards times are properly ordered.
        let mut o = Orderer::new();
        for i  in 0..10 {
            o.add_hit(true, i as u16 % 2 , (9-i) as u64, 666);
        }
        println!("{:?}", o.hits);
        let ordered = o.order();
        assert_eq!(ordered.len(), 10);
        println!("{:?}", ordered);
        for i in 0..10 {
            assert_eq!(ordered[i].2,  i as u64);
        }
    }
    #[test]
    fn order_5() {
        // put a few random hit times in...they come out ordered.

        let mut o = Orderer::new();
        let mut times : Vec<u64> = Vec::new();   // Store generated times here.
        let mut r = rand::rng();
        for _ in 0..50 {
            let t : u64 = r.random();
            times.push(t);
            o.add_hit(true, 0, t, 666);
        }
        times.sort();
        let ordered = o.order();
        assert_eq!(ordered.len(), 50);
        for i in 0..50 {
            assert_eq!(ordered[i].2, times[i]);
        }
    }
}