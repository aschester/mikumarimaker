use mikumarimaker::mikumari_format;

use std::io::{stdin, BufReader, Read};
use std::fs::File;
use rust_ringitem_format::{RingItem, BodyHeader, ToRaw};
use rust_ringitem_format::state_change::{StateChange, StateChangeType};  // begin run/end run.
use frib_datasource::{data_sink_factory, DataSink};

use clap::{value_parser, Arg, ArgAction, Command, ArgMatches};
use std::time;


const HEART_BEAT_MICROSECONDS : f64 = 524.288; // Time between heart beats.
const TDC_TICK_PS : f64 = 0.9765625;           // LSB value for tdc.

/// We're going to support the following optional uhm.. options.
/// --title - a run title.
/// --run   - a run number.
/// --source-id -an event source id.
///

fn main() ->std::io::Result<()> {

    let parser = Command::new("mikumarimaker")
        .version("0.1.1")
        .about("Make raw mikumari data into frame ring items")
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

    // Let's get the title, run number and source id given the arguments

    let title = get_title(&matches);
    let run_num = get_run(&matches);
    let sid     = get_source_id(&matches);
    
    
    let fname = matches.get_one::<String>("source").expect("Source filename is required").clone();
    let ring_name = matches.get_one::<String>("sink").expect("Sink URI is required").clone();

    // Open the file, attach a buffered reader to it and box it to create
    // a MikumariReader:

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
    
    // Open the output ring item - or ring buffer.

    let mut ring_file = data_sink_factory(&ring_name).expect("Unable to open data sink"); 

    // Set up to encapsulate the run:

    let begin_run_time = time::Instant::now();  // start time of the run.
    let mut b       = BodyHeader {
        timestamp: 0xffffffffffffffff,       // EVB assign timestamp.
        source_id : sid,
        barrier_type: 1                     // begin run barrier.
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
    b.barrier_type = 2;                          // end run barrier.
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

//  * sid  - user source id, stamped on every frame item.
// Ring items are built by buffering hits and emitting them when the trailing heartbeat that 
// closes the frame arrives. That heartbeat's frame number labels and timestamps every hit 
// that preceded it.
//  * The ring item body looks like: [absolute frame number : u64][raw hit 0: u64]...
//  * The body-header timestamp is relative (first emitted frame = 0) and is
//    advanced by the real heartbeat-to-heartbeat frame delta, so dropped
//    frames and the 24-bit frame-number rollover are handled correctly.
//  * Hits after the final heartbeat have no closing heartbeat, so that trailing
//    partial frame cannot be timestamped and is discarded at EOF.
fn dump_data(src: &mut mikumari_format::MikumariReader, sid: u32,
             rf: &mut Box<dyn DataSink>) {
    let mut buf: Vec<u64> = Vec::new();        // raw hit words for the open frame
    let mut first_frame: Option<u64> = None;   // frame no. of the first heartbeat
    let mut prev_frame: u64 = 0;               // previous heartbeat's 24-bit frame number
    let mut rel_frame: u64 = 0;                // relative frame index (0-indexed)

    while let Ok(data) = src.read() {
        match data {
            // Accumulate hits (raw words already carry channel, TOT, within-frame time).
            mikumari_format::MikumariDatum::LeadingEdge(le)  => buf.push(le.get()),
            mikumari_format::MikumariDatum::TrailingEdge(te) => buf.push(te.get()),

            // Trailing heartbeat: it closes the frame these buffered hits belong to.
            mikumari_format::MikumariDatum::Heartbeat0(d1) => {
                let current_frame = d1.frame();      // 24-bit trailing frame number
                match first_frame {
                    None => {                        // first heartbeat -> relative frame 0
                        first_frame = Some(current_frame);
                        rel_frame = 0;
                    }
                    Some(_) => {                     // advance by the real frame delta
                        let delta = current_frame.wrapping_sub(prev_frame) & 0xffffff; // drops + rollover
                        if delta != 1 {              // warn if non-consecutive frame numbers (stderr)
                            eprintln!(
                                "WARNING: non-consecutive frame: prev={} current={} delta={} (expected 1)",
                                prev_frame, current_frame, delta
                            );        
                        }
                        rel_frame += delta;
                    }
                }
                prev_frame = current_frame;

                let abs_frame = first_frame.unwrap() + rel_frame;   // non-rolling u64
                let mut item = RingItem::new_with_body_header(
                    mikumari_format::MIKUMARI_FRAME_ITEM_TYPE,
                    hb_frame_to_ts(rel_frame) as u64,
                    sid, 0,
                );
                item.add(abs_frame);
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

// Convert a frame number to a mikumari timestamp:

fn hb_frame_to_ts(frame: u64) -> f64 {
    let frame_t : f64 = frame as f64 * HEART_BEAT_MICROSECONDS; // frame_time in usec.
    (frame_t * (1.0e6)) / TDC_TICK_PS
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
