use mikumarimaker::{mikumari_format, glom};

use std::mem::size_of;
use clap::{value_parser, Arg, ArgAction, Command};

use rust_ringitem_format::{RingItem, BEGIN_RUN, END_RUN};
use frib_datasource::{data_source_factory,  data_sink_factory};

/// Ring items generated are PHYSICS_EVENT. The body is zero or more hits:
/// | channel/edge | u16 — top bit set for a trailing edge
/// | absolute time| u64 — hit time + the frame item's timestamp
/// | TOT          | u32 — time over threshold
/// Frame boundaries appear as a pseudo-hit: channel 0xffff, time = absolute
/// frame number, TOT = 0xffffffff.
///
/// If I've done arithmetic properly, it's 213 days before the absolute time 
/// should wrap.
///
/// Usage:
///    defenestrator --dt coincidence-interval source sink.
///
/// Source and sink are URIs, --dt is in TDC ticks.
///
fn main() {
    // Define the command line parameter for clap:
    let parser = Command::new("defenestrator")
        .version("0.3.0").about("Defenestrates Mikumari time data")
        .arg(Arg::new("dt")
            .short('t').long("dt").required(true).help("Coincidence interval (ticks)")
            .action(ArgAction::Set)
            .value_parser(value_parser!(u64))
        )
        .arg(Arg::new("source").required(true).help("Data Source URI"))
        .arg(Arg::new("sink").required(true).help("Data Sink URI"));

    let matches = parser.get_matches();

    // Process the command line arguments:

    let ring_uri = matches.get_one::<String>("source").expect("No data source given");
    let out_path = matches.get_one::<String>("sink").expect("No data sink given");
    let glom_dt = matches.get_one::<u64>("dt").expect("No --dt given for glomming");

    // Open the source:

    let mut source = data_source_factory(&ring_uri).expect("Could not open ring item source");
    let sink   = data_sink_factory(&out_path).expect("Could not open ring item sink");
    
    // Create the glommer:

    let mut glom = glom::Glom::new(sink, 0, *glom_dt);

    // Process the items:

    while let Some(item) = source.read() {
        convert_item(&item, &mut glom);
    }

    // In case there's no EOF, flush glom:

    glom.flush()

}

fn convert_item(item : &RingItem, glom  : &mut glom::Glom) {
    // If the ring item is not a MIKUMARI frame, just pass it unaltered.
    let item_type = item.type_id();
    if item_type != mikumari_format::MIKUMARI_FRAME_ITEM_TYPE {
        // Some are special:        
        if item_type == BEGIN_RUN {
            // Set the source id from the ring item:
            if let Some(bh) = item.get_bodyheader() {
                glom.set_sid(bh.source_id);
            }
        } else if item_type == END_RUN { // Flush event being built at end run.
            glom.flush();
        }
        glom.write_item(item)
    } else {
        let bh = item.get_bodyheader().unwrap();
        let t0 = bh.timestamp;
        let payload = item.payload();    // Vec<u8>
        glom.set_sid(bh.source_id);      // In case there's no BEGIN e.g.
        
        // We are assured there's an absolute frame number (64 bits).
        // Payload includes the body header.

        let mut cursor = size_of::<u64>() + 2 * size_of::<u32>(); // Skip body header.
        let absolute_fno = u64::from_ne_bytes(payload[cursor..cursor+size_of::<u64>()].try_into().unwrap());

        // Sort the hits and add them to the glommer:

        let mut orderer = glom::Orderer::new();

        cursor += size_of::<u64>();      // First (if any) data item:
        while cursor < payload.len() {
            let raw = u64::from_ne_bytes(payload[cursor..cursor+size_of::<u64>()].try_into().unwrap());            
            match mikumari_format::MikumariDatum::from_u64(raw) {
                mikumari_format::MikumariDatum::LeadingEdge(le)  => {
                    let t : u64 = le.time() as u64 + t0;
                    orderer.add_hit(true, le.channel() as u16, t, le.tot());
                    
                },
                mikumari_format::MikumariDatum::TrailingEdge(te) => {
                    let t : u64 = te.time() as u64 + t0;
                    orderer.add_hit(false, te.channel() as u16, t, te.tot());
                },
                _ => {},                 // Anything else is not passed through.
            }
            cursor += size_of::<u64>();
        }

        // Get hits from the orderer and put them in glom:

        let merged_hits = orderer.order();
        for (rising, chan, time, tot) in &merged_hits {
            glom.add_hit(*rising, *chan as u8, *time, *tot);
        }

        // Trailing frame boundry: ...[frame f hits][marker f][frame f+1 hits]...

        glom.add_frame_boundary(absolute_fno);
    }
}