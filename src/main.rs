use mikumarimaker::mikumari_format;

use std::io::{stdin, BufReader, Read};
use std::fs::File;
use clap::{value_parser, Arg, ArgAction, Command, ArgMatches};
use std::time;

use rust_ringitem_format::{RingItem, BodyHeader, ToRaw};
use rust_ringitem_format::state_change::{StateChange, StateChangeType};  // begin run/end run.
use frib_datasource::{data_sink_factory, DataSink};

// 524.288 us/frame * 10^6 us/ps / 0.9765625 ps/tick = 2^29 ticks/frame
const TICKS_PER_HB_FRAME: u64 = 1 << 29;

/// We're going to support the following optional uhm... options.
/// --title     - a run title.
/// --run       - a run number.
/// --source-id - an event source id.
fn main() ->std::io::Result<()> {
    let parser = Command::new("mikumarimaker")
        .version("0.3.0")
        .about("Make raw Mikumari data into frame ring items")
        .arg(Arg::new("title").short('t').long("title").action(ArgAction::Set)
            .required(false).default_value("No title set")
        ).arg(Arg::new("run").short('r').long("run").action(ArgAction::Set)
            .required(false).default_value("0")
            .value_parser(value_parser!(u32))
        )
        .arg(Arg::new("source-id").short('s').long("source-id").action(ArgAction::Set)
            .required(false).default_value("0")
            .value_parser(value_parser!(u32))
        )
        .arg(Arg::new("source").required(true).action(ArgAction::Set))
        .arg(Arg::new("sink").required(true).action(ArgAction::Set));
    let matches = parser.get_matches();

    // Let's get the title, run number and source id given the arguments:

    let title = get_title(&matches);
    let run_num = get_run(&matches);
    let sid = get_source_id(&matches);    
    
    let fname = matches.get_one::<String>("source").expect("Source filename is required").clone();
    let ring_name = matches.get_one::<String>("sink").expect("Sink URI is required").clone();

    // Open the file, attach a buffered reader to it and box it to create
    // a MikumariReader data source:

    let source : Box<dyn Read> = 
    if fname == "-" {
        let inf = stdin();
        Box::new(inf)
    } else {
        let f = File::open(&fname)?;
        let reader = BufReader::new(f);
        Box::new(reader)
    };

    let mut data_source = mikumari_format::MikumariReader::new(source);
    
    // Open the output ring item - or ring buffer:

    let mut ring_file = data_sink_factory(&ring_name).expect("Unable to open data sink"); 

    // Set up to encapsulate the run:

    let begin_run_time = time::Instant::now();  // Start time of the run.
    let mut b = BodyHeader {
        timestamp: 0xffffffffffffffff,          // Null timestamp for begin run item.
        source_id : sid,
        barrier_type: 1                         // Begin run barrier.
    };
    let begin_run = StateChange::new_with_body_header(
        StateChangeType::Begin,
        &b,
        run_num, 0, 1, &title, Some(sid)
    );
    ring_file.write(&begin_run.to_raw()).expect("Failed to write begin run item to sink.");

    // Start accumulating and writing data from the source to the ring item sink. 
    // The run is encapsulated by the begin and end run items.

    dump_data(&mut data_source, sid, &mut ring_file);

    // The end run item:

    let elapsed = begin_run_time.elapsed();
    b.barrier_type = 2;                          // End run barrier.
    let end_run = StateChange::new_with_body_header(
        StateChangeType::End,
        &b,
        run_num, elapsed.as_secs() as u32,
        1, &title, Some(sid)
    );
    ring_file.write(&end_run.to_raw()).expect("Failed to write end run item to sink");
    ring_file.flush();     // Probably not needed but what the heck.
    Ok(())
}

/// Build frame ring items by buffering hits and emitting them when the
/// trailing heartbeat that closes the frame arrives: that heartbeat's frame
/// number labels and timestamps every hit that preceded it.
///
/// * `sid` - user source id, stamped on every frame item.
/// * Body layout: `[mikumari frame number : u64 - raw, rolls at 2^24][raw hit 0 : u64]...`
/// * The body-header timestamp is relative (first emitted frame = 0), advanced
///   by the real heartbeat-to-heartbeat frame delta, so dropped frames and the
///   24-bit frame-number rollover are handled correctly.
/// * Hits after the final heartbeat have no closing heartbeat, so that trailing
///   partial frame is discarded at EOF.
fn dump_data(src: &mut mikumari_format::MikumariReader, sid: u32,
             rf: &mut Box<dyn DataSink>) {
    let mut buf: Vec<u64> = Vec::new();        // Raw hit words for the open frame.
    let mut seen_first = false;                // True after the first heartbeat.
    let mut prev_frame: u64 = 0;               // Previous heartbeat's 24-bit frame number.
    let mut rel_frame: u64 = 0;                // Relative frame index (0-indexed).

    while let Ok(data) = src.read() {
        match data {
            // Accumulate hits (raw words already carry channel, TOT, within-frame time).
            mikumari_format::MikumariDatum::LeadingEdge(le)  => buf.push(le.get()),
            mikumari_format::MikumariDatum::TrailingEdge(te) => buf.push(te.get()),

            // Trailing heartbeat: it closes the frame these buffered hits belong to.
            mikumari_format::MikumariDatum::Heartbeat0(d1) => {
                let current_frame = d1.frame();      // 24-bit trailing frame number.
                if !seen_first {
                    seen_first = true;               // First heartbeat -> rel_frame stays 0
                } else {
                    // 24-bit wrapping subtraction: 0xffffff -> 0x000000 is a delta of 1, not 0xfffffe.
                    let delta = current_frame.wrapping_sub(prev_frame) & 0xffffff;
                    if delta != 1 {
                        eprintln!("WARNING: non-consecutive frame: prev={} current={} delta={} (expected 1)",
                                prev_frame, current_frame, delta);
                    }
                    rel_frame += delta;
                }
                prev_frame = current_frame;

                let mut item = RingItem::new_with_body_header(
                    mikumari_format::MIKUMARI_FRAME_ITEM_TYPE,
                    hb_frame_to_ts(rel_frame),
                    sid, 0,
                );
                item.add(current_frame);
                for w in &buf {
                    item.add(*w);
                }
                rf.write(&item).expect("Failed to write a ring item to data sink.");
                buf.clear();
            }

            // Delimiter 2 and everything else carry no hit data for us.
            mikumari_format::MikumariDatum::Heartbeat1(_d) => (),
            mikumari_format::MikumariDatum::Other(_d)      => (),
        }
    }
    // Whatever is still in `buf` came after the last heartbeat: an unclosed
    // partial frame with no frame number and therefore no timestamp. It is 
    // discarded at EOF.
}

/// Convert a frame number to a Mikumari timestamp.
/// ### Parameters
/// * `frame` - The relative frame number (0-indexed) to convert to a timestamp.
/// ### Returns
/// The timestamp in clock ticks (1/1024 ns per tick) corresponding to the
/// given frame number.
fn hb_frame_to_ts(frame: u64) -> u64 {
    frame * TICKS_PER_HB_FRAME // Frame time in clock ticks.
}

fn get_title(parsed : &ArgMatches) -> String {
    parsed.get_one::<String>("title").expect("there should have been a default title").clone()
}

fn get_run(parsed : &ArgMatches) -> u32 {
    let result : u32 = *parsed.get_one::<u32>("run").expect("there should be a default run number");
    result
}

fn get_source_id(parsed: &ArgMatches) -> u32 {
    *parsed.get_one::<u32>("source-id").expect("There should be a default source-id")
}

#[cfg(test)]
mod dump_data_tests {
    use super::*;
    use std::io::Cursor;
    use std::rc::Rc;
    use std::cell::RefCell;

    // Records each written frame item: timestamp, source id, and body decoded
    // as u64 words -> [abs frame number][raw hit 0][raw hit1]...
    #[derive(Clone)]
    struct Captured { timestamp: u64, source_id: u32, body: Vec<u64> }
    struct CapSink { items: Rc<RefCell<Vec<Captured>>> }
    impl DataSink for CapSink {
        fn open(&mut self, _uri: &str) -> Result<(), String> { Ok(()) }
        fn write(&mut self, item: &RingItem) -> Result<(), String> {
            let bh = item.get_bodyheader().unwrap();
            let p = item.payload();                       // includes 16-byte body header
            let body = p[16..].chunks_exact(8)
                .map(|c| u64::from_ne_bytes(c.try_into().unwrap()))
                .collect();
            self.items.borrow_mut().push(Captured {
                timestamp: bh.timestamp, source_id: bh.source_id, body,
            });
            Ok(())
        }
        fn close(&mut self) {}
        fn flush(&mut self) {}
    }

    fn hit(ch: u8, tot: u32, time: u32) -> u64 {
        mikumari_format::HRTDCLeading::new(ch, tot, time).get()
    }
    fn hb(frame: u32) -> u64 {
        mikumari_format::Delimeter1::new(0, frame).get()
    }
    // Run dump_data over a word stream, return what the sink captured.
    fn run(words: &[u64], sid: u32) -> Vec<Captured> {
        let mut bytes = Vec::new();
        for w in words { bytes.extend_from_slice(&w.to_ne_bytes()); }
        let mut reader = mikumari_format::MikumariReader::new(Box::new(Cursor::new(bytes)));
        let cap = Rc::new(RefCell::new(Vec::new()));
        let mut sink: Box<dyn DataSink> = Box::new(CapSink { items: cap.clone() });
        dump_data(&mut reader, sid, &mut sink);
        cap.borrow().clone()
    }

    // ---- trailing heartbeat terminates a frame ----

    // Hits are grouped with the heartbeat that FOLLOWS them; hits before the
    // first heartbeat are emitted (not skipped); sid is stamped on the item.
    #[test]
    fn hits_grouped_with_trailing_heartbeat() {
        let (a, b) = (hit(0, 10, 100), hit(1, 11, 101));   // belong to frame 100
        let (c, d) = (hit(0, 12, 200), hit(1, 13, 201));   // belong to frame 101
        let items = run(&[a, b, hb(100), c, d, hb(101)], 7);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].source_id, 7);
        assert_eq!(items[0].body, vec![100, a, b]);        // [abs frame, hits...]
        assert_eq!(items[1].body, vec![101, c, d]);
    }

    // Hits after the last heartbeat have no closing heartbeat -> discarded.
    #[test]
    fn hits_after_last_heartbeat_dropped() {
        let (a, b) = (hit(0, 10, 100), hit(1, 11, 101));
        let (c, d) = (hit(0, 12, 200), hit(1, 13, 201));   // no trailing hb
        let items = run(&[a, b, hb(100), c, d], 7);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].body, vec![100, a, b]);        // c, d dropped
    }

    // ---- delta calculation with rollover protection ----

    // Consecutive frames: abs frame number (body[0]) and the relative timestamp
    // each advance by exactly one frame.
    #[test]
    fn consecutive_frames_increment_by_one() {
        let items = run(&[hit(0,1,1), hb(100),
                          hit(0,1,2), hb(101),
                          hit(0,1,3), hb(102)], 7);
        assert_eq!(items.len(), 3);
        assert_eq!((items[0].body[0], items[0].timestamp), (100, 0));
        assert_eq!((items[1].body[0], items[1].timestamp), (101, TICKS_PER_HB_FRAME));
        assert_eq!((items[2].body[0], items[2].timestamp), (102, 2 * TICKS_PER_HB_FRAME));
    }

    // Missing frame (100 -> 102): rel_frame += delta, so the gap shows up as a
    // full frame jump in both number and timestamp. (The stderr warning is
    // visible with `cargo test -- --nocapture`.)
    #[test]
    fn missing_frame_leaves_a_gap() {
        let items = run(&[hit(0,1,1), hb(100),
                          hit(0,1,2), hb(102)], 7);          // 101 dropped
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].body[0], items[0].timestamp), (100, 0));
        assert_eq!((items[1].body[0], items[1].timestamp), (102, 2 * TICKS_PER_HB_FRAME)); // +2, not +1
    }

    // 24-bit rollover 0xffffff -> 0x000000: the `& 0xffffff` masks the wrap to a
    // delta of 1, so the relative counter advances one frame with no false
    // gap/warning. The body stores the raw Mikumari frame number, so it wraps to 0.
    #[test]
    fn frame_rollover_is_one_step() {
        let items = run(&[hit(0,1,1), hb(0xffffff),
                          hit(0,1,2), hb(0x000000)], 7);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].body[0], 0xffffff);
        assert_eq!(items[0].timestamp, 0);
        assert_eq!(items[1].body[0], 0x000000);             // Raw frame wraps.
        assert_eq!(items[1].timestamp, TICKS_PER_HB_FRAME); // delta == 1 -> timestamp advances one frame.
    }
}