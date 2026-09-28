use mikumarimaker::mikumari_format;
use std::process::exit;

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

    // Mikumari data has a partial frame at the front. We _could_
    // figure out how to timestamp it, but, instead, we'll just skip
    // that data as that seems to be standard.

    let hb = skip_partial_frame(&mut data_source);

    let hb_t0 = hb.frame();        // our t0 frame.
    dump_data(&mut data_source, hb_t0, sid, &mut ring_file);

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
fn skip_partial_frame(src : &mut mikumari_format::MikumariReader) ->
    mikumari_format::Delimeter1
{
    
    while let Ok(data) = src.read() {
        if let mikumari_format::MikumariDatum::Heartbeat0(d1) = data {
            return d1;
        }
    }
    // We had an error before finding a heartbeat.

    eprintln!("Did not find the first heartbeat before eof or read error");
    exit(-1);

}
// t0 - the frame # of t0.
// sid - the user-provided source id.
// We're going to try to make the times into absolutes as well.
// Ring items we make:
//   These consist of raw hit values.
//   the timestamp comes from the relative frame_no, but the first
//   u64 bit item is the absolute frame number.
//
fn dump_data(src : &mut mikumari_format::MikumariReader, t0 : u64, sid: u32, rf : &mut Box<dyn DataSink>) {
    let mut frame_no = 0;                       // THe current frame number.
    let mut absolute_frame = t0;

    // start a ring item for the first frame:

    let mut ring_item = RingItem::new_with_body_header(
        mikumari_format::MIKUMARI_FRAME_ITEM_TYPE,
        hb_frame_to_ts(frame_no) as u64,
        sid, 0
     );
     ring_item.add(absolute_frame);
    while let Ok(data) = src.read() {
        match data {
            mikumari_format::MikumariDatum::LeadingEdge(le) => {
                ring_item.add(le.get());  // ISsue #11 Already includes the TOT field.
            },
            mikumari_format::MikumariDatum::TrailingEdge(te) => {
                ring_item.add(te.get());  // ISsue #11 Already includes the TOT field.
            }
            mikumari_format::MikumariDatum::Heartbeat0(_d) => {
                // Heart beat means we write the item and 
                // start a new one:
                rf.write(&ring_item).expect("Failed to write a ring item to data sink.");
            
                frame_no += 1;                   // Next frame.
                absolute_frame += 1;
                // Start the new ring item:

                ring_item = RingItem::new_with_body_header(
                    mikumari_format::MIKUMARI_FRAME_ITEM_TYPE,
                    hb_frame_to_ts(frame_no) as u64,
                    sid,0
                );
                ring_item.add(absolute_frame);
            }
            mikumari_format::MikumariDatum::Heartbeat1(_d) => (),
            mikumari_format::MikumariDatum::Other(_d) => (),
        }
    }
    // Flush the last ring item out:
  
    rf.write(&ring_item).expect("Failed to write ring item to data sink.");
    
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
