# What's here:

This repository is prototypical code to handle the Mikumari time synchronized system with HRTDC daughterboards.
The work closely follows the brainstorming meeting the FRIBDAQ group did at the beginning of 2026.  

## Building the code:

You will need the Rust language toolchain. At the FRIB, in our containers you can make that toolchain visible by:
```bash
export PATH=/usr/cargo/bin:$PATH
```

Clone this repository or grab a release version of the repository and make your current working directory the top level directory of the repository.

To build for development/debugging:

```bash
cargo build
```

The executables, mikumarimaker and defenestrator will be created in ```target/debug```.

To build for production:
```bash
cargo build --release
```

the executables, mikumarimaker and defenestrator will be created in ```target/release```.

From there you can install them anywhere you want or just run them from those directories.

### Running unittests

To run the test suite for this program, run the command 

```bash
cargo test
```

from the top-level directory. All tests should pass.

## Products of this repository:

Two binaries here:
*  mikumarimaker takes a raw Mikumari time data file and makes a ring item file.
*  defenestrator takes the output of e.g. mikumarimaker and output defenestrated ring items.

###  mikumarimaker

This program takes raw Mikumari High-Precision TDC frame files and produces ring item frame files.
These have the following characteristics:

*  Hits before the first heartbeat are kept — the first heartbeat closes and timestamps them.
*  Hits after the last heartbeat have no closing heartbeat and are discarded. Generally occurs at the end of a run.
*  The item type is 51  - time frames.
*  The timestamp in the body header is the computed timestamp of the heartbeat that closes the frame. By computed timestamp I mean 
   that the first frame number is assigned a timestamp of 0. Subsequent frames have a timestamp that is the 
   frame number * the number of TDC ticks per frame. This assumes that (as documented):
    - Frames are 524.288&mu;seconds apart.
    - The TDC tick (LSB resolution) is 0.9765625 picoseconds.
   This gives a value of 2^29 ticks per frame (= 524.288 &mu;s/frame * 10^6 &mu;s/ps / 0.9765625 ps/tick)

Note: the frame number relative to the start of data are internally maintained as a uint64_t. The absolute frame number will roll over 
every ~2.44 hours, but the timestamp generated from the relative frame number will roll over every 2^64/2^29 = 2^35 &mu;s or ~208 days.

The ring item body contains the following:

|  Contents      | Size       |  Notes                                          | 
|----------------|------------|-------------------------------------------------|
| Raw frame number | uint64_t | Only the least significant bits are meaningful. |
| Raw hit 0        | uint64_t | The raw TDC value (rising or falling edges). |
| Raw hit 1        | uint64_t | The raw TDC value (rising or falling edges). |
|    ...           |   ...    | ... |

Where there will be as many raw hit values as there are up to the next heartbeat.
The following are filtered out:
* The frame number is extracted from the delimiter 1 word and stored, delimiter 1 word is discarded.
* Heartbeat delimiter 2 word.
* Throttle words. Note that these could easily be added back if desired.

Usage of the program:

```
mikumarimaker [options] infile outuri
```
* Options control the begin/end run items that bracket the data.  See OPTIONS 
below for what they are and the default values. 
* infile is the path to the file that contains raw mikumari data.
* outuri is the URI of the output this can be a file: or tcp://localhost/ring_name
for online data.

#### OPTIONS

The program accepts several command line options to control what goes in the begin run
and end run items that bracket the data:

| Long form   |  Short form | Default              | Meaning of value                           |
|-------------|-------------|----------------------|--------------------------------------------|
| --title     | -t          | ```"No title set"``` | Title string in begin and end run items |
| --run       | -r          | ```0```              | Run number in begin and end run items |
| --source-id | -s          | ```0```              | Source id put in body headers. |
| --version   | -v          | N/A                  | outputs the program version and exits |
| --help      | -h          | N/A                  | outputs brief program usage help and exits |

### defenestrator

Defenestration means to throw someone out a window.  In the context of FRIB/NSCLDAQ, it means to take windowed (frame files) and 
turn them into 'something else'.  For time data, that 'something else' is hits accumulated into events based on a settable coincidence 
interval.  Since frame times are not considered to be precise, the position of the frames in the data are retained.

The program passes all ring items that are not Mikumari time frames
(type 51) through without modification.

The program can accept data from an input file, stdin, or ringbuffer and write data to an output file or stdout.  Future work may allow 
this to send output to an online ringbuffer.

The output of this program are a sequence of ring items.

*  The type of the ring items is ```PHYSICS_EVENT```
*  The ring items have body headers
*  Each PHYSICS_EVENT ring item is a set of hits that occured within a settable coincidence interval
*  Event ring items have body headers:
    * The timestamps on the body headers are the time of the first hit.
    * The source id on the body headers is the source id of the input data.
*  A special hit channel identifies where frame boundaries are.

The defenestrator outputs what it thinks are events given a coincidence interval.  Each event will have a timestamp derived from the first hit 
in the event.  Hits consist of a 16-bit channel/edge word followed by a 64-bit absolute time word followed by a 32-bit time over threshold:


|  Contents       | Size     | Notes     |
|-----------------|----------|-----------|
| Channel and edge| uint16_t | The top bit is set for falling edge, the remainder of the word is the channel number.
| Absolute time   | uint64_t | Time of the hit relative to the start of the run. |
| TOT             | uint32_t | Time over threshold. |

Frame boundaries are shown by a hit with the channel and edge field, and TOT set to 0xffffffff.  The "_time_" of that hit is the absolute frame number. For example:

```
0xffff
0x0000000000001000
0xffffffff
```

is a frame boundary with the absolute frame number 4096.  Note that the data are little endian so for this example in 16-bit words in the dumper will be in 
the following order:

```
0xffff   - Frame boundary flag.
0x1000  \   Least significant 32 bits
0x0000  /   of the frame number
0x0000  \   Most significant 32  bits
0x0000  /   of the frame number.
0xffff  -   Least significant bits of fake time over threshold.
0xffff  -   Most significant bits of fake time over threshold.
```

Note the absolute times of actual hits are computed from the mikumari hit time and the timestamp of the input ring item that contained them (see mikumarimaker).  
It will roll over after over 208 days and the LSB as for the ring item timestamp is 0.9765625 ps.

The timestamp of the input ring items (from mikumarimaker) are the computed time of the frame relative to the first frame. For example times in the 0th frame 
will not be altered, while times in the second frame will have

```
524.288 usec/frame* 10^6 ps/usec / 0.9765625 ps/tdc-tick = 2^29 tdc-ticks
```

added to them.

Usage:
```
defenestrator --dt coincidence-window source-uri out-uri
```

Where:
|  parameter | Meaning                    |
|------------|----------------------------|
| source-uri | The data source URI as per standard FRIB/NSCLDAQ URI format |
| sink-uri | The URI specifying either the file or ring buffer to which data are written |
| --dt     | The argument of this option is the coincidence window in TDC Ticks |

source and sink URIS  can have the form:

* file:///absolute-path-to-some-file for  file data.
* tcp://hostname/ringname for ringbuffers. Note that output-uris must use ```localhost`` for the hostname
* file://- is a special path that for source_uri's means stdin and for sink-uris stdout.

#### Sample SpecTcl Code.

For data taken during the test runs, I have made a SpecTcl event processor for the scintillator data.  These data contain hits from two channels.  A north and South PM on the scintillator.  The event processor produces time differences within a frame, time differences that cross frame boundaries, North and South time over threshold parameters and a crude position spectrum using the time over thresholds and light division.

This code is intended as an example rather than production code.

The header: miku_decoder.h

```c++
#ifndef MIKU_DECODER_H
#define MIKU_DECODER_H
#include <EventProcessor.h>
#include <TreeParameter.h>

class CEvent;
class CAnalyzer;
class CBufferDecoder;
class CTreeParameter;
class CTreeParameterArray;

class Miku_diffs : public CEventProcessor {
    CTreeParameter diff;      // Time diff in a frame.
    CTreeParameter diff_cross;      // Time diff across frames.
    CTreeParameterArray tots;  // Time over thresholds chan0, chan1.
    CTreeParameter position;
 public:
    Miku_diffs();
    virtual Bool_t operator()(const Address_t pEvent,
                            CEvent& rEvent,
                            CAnalyzer& rAnalyzer,
                            CBufferDecoder& rDecoder); 
 
};

#endif
```

The implementation of that class is: miku_decoder.cpp:

```c++
#include "miku_decoder.h"
#include <stdint.h>
#include <vector>
#include <BufferDecoder.h>

/* constructor */

Miku_diffs::Miku_diffs() :
    diff("diff"),
    diff_cross("diff-cross"),
    tots("TOT", 2, 0),
    position("Pos")
     {}

/**
 * Decode the data we have data of the form
 * |fall| chan | (16 bits)
 * | time      | (64 bits)
 * 
 * where a first 16 bit word of 0xffff means a frame boundary.
 * We're going to fill iin diff from differences that don't cross frame
 * boundaries and diff_cross with those that do.
 * We assume there are at most 2 actual hits.
 */
Bool_t
Miku_diffs:: operator()(const Address_t pEvent,
                            CEvent& rEvent,
                            CAnalyzer& rAnalyzer,
                            CBufferDecoder& rDecoder) {
    // This union has 16 and 64  bit pointers.

    union {
        uint16_t* pw;
        uint32_t* pl;
        uint64_t* pq;
    } p;
    p.pw = static_cast<uint16_t*>(pEvent);
    bool crosses = false;
    std::vector<uint64_t> hits;    // Order(0) don't care about the channels:
    UInt_t n = rDecoder.getBodySize();


    while(n) {
        uint16_t header = *p.pw++;
        uint64_t time   = *p.pq++;
        uint32_t tot    = *p.pl++; 
        n -= sizeof(uint16_t) + sizeof(uint64_t) + sizeof(uint32_t);
        if (header == 0xffff) {
            // frame boundary
            if (hits.size() > 0) {
                crosses = true;        // Hits cross boundary.
            }
        } else {
            hits.push_back(time);
            uint16_t chan = header & 0x7fff;
            if (chan < 2) {
                tots[chan] = tot;
            }
        }
    }
    
    if (hits.size() >= 2) {
        uint64_t tdiff = hits[1] - hits[0];   // time difference.
        if (crosses) {
            diff_cross = double(tdiff);
        } else {
            diff = double(tdiff);
        }
        position = 1024*(tots[0] - tots[1])/(tots[0] + tots[1]);

    }

    return kfTRUE;
}
```