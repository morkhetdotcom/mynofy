<a id="gemeinsames-binäres-nachrichtenlayout"></a>

# Common binary message layout

The message stock uses an initialized, pointer-free byte layout both in the RAM and in a message
image. Saving and reading do not convert records, payload blocks and content type table into
JSON, prototype or native Rust objects. File frames and checksums are additionally generated or
checked. All own accesses remain without `unsafe`.

The conversion affects the running core, not just an export encoder. The direct Protobuf-Poll
encoder reads the same byte records and payload blocks. JSON and the typed Protobuf-API form
answers as needed. The existing Such-/Sortier-/expiration indexes remain separate runtime structures.

## Record v1

A record occupies exactly **80 bytes**, including a 32-byte payload union.A occupied arena slot
has the same 80 bytes, with no `Option<Record>` or native enum or padding representation. multibyte
numbers being Little Endian; message IDs contain eight ASCII bytes from the existing alphabet.

| Offset | Width | Field |
| --- | --- | --- |
| 0 | 8 | Message-ID |
| 8 | 8 | Absolute expiration time, Unix-ms |
| 16 | 8 | Producer-Annahmeposition |
| 24 | 8 | Topic-Annahmeposition |
| 32 | 8 | Complete creation time, Unix-ms |
| 40 | 2 | Producer-Registry-Slot |
| 42 | 2 | Topic-Registry-Slot |
| 44 | 4 | Content type ID, 0 = none |
| 48 | 32 | Payload-Union |

The complete time of creation replaces the old U32 time component together with a special table
for long TTLs, whose diagnostic field `extended_time_entries` is retained for the time being
with 0. There is no time-dependent reinterpretation of stored records.

The Union contains its type mark at relative offset 6, reserved byte 7 is 0:

- **Bytes, number 1:** Length u32 at 0; reserved bytes 4–5 are 0; relative block address u64 at
  8; bytes 16–31 are 0. Length maximum 4096. Empty payloads have address 0 and do not need a
  block.
- **Event, License 2:** Category U16/Aktion U8/Status U8 as common u32 key at 0; three 3-bit
  parameter types as u16 at 4; 24 value bytes from 8. Unused Bits/Bytes are 0. The built-in
  catalog 1/v1 remains implicit. Type sequence, bool values and finite floats are checked upon
  reading in; float bits including negative zero value are retained.

Records are in byte chunks to 256 slots: live counter u32 at 0, four reserved zero bytes, 32
bytes occupancy bitmap and then 256 × 80 record bytes. The bitmap is the occupancy feature; free
records are completely 0. The total persisted chunk size is 20,520 bytes. Free lists and chunk
search positions do not belong to this block.

<a id="variable-daten-und-inhaltstypen"></a>

## Variable data and content types

Byte payloads and internal content type names are located in separate pages with **16 KiB data
range**. Size classes from 16 to 4096 bytes allow for reuse of free spaces without moving living
payloads. One address contains `(Seiten-ID + 1)` in the top 32 bits and the offset within the
data range in the bottom 32 bits. It does not contain a process pointer.

A page starts with class log2 (1 byte), three reserved zero bytes, live counter u32 and a
class-matched bitmap of u64 words, the rest is the data area, so the page size is 16 KiB plus
16-136 headerbytes, only the required bitmap is stored, unused slots and the rest of a occupied
class space according to the actual data length is 0.

The content type table consists of 16-byte entries: block address u64, UTF -8 length u32 and
reference number u32. Free entries are 0. Your name search is a volatile hash index; when read,
it is reconstructed from the tested bytes. The unchanged content type ID connects Records to
this table.

When a message is removed, record and shared block spaces are completely zeroed out. Old
contents in already shared spaces are not inadvertently backed up. Empty pages and chunks are
released budgeted during the existing maintenance; free spaces remain reusable beforehand. New
diagnostic fields `payload_allocated_bytes`, `payload_pages` and `content_type_table_bytes` make
page capacity and table costs visible. The logical acceptance rate is still not an exact RSS
limit.

The previous individual `Bytes` allocation is no longer part of the permanent record. The direct
wire encoder writes from borrowed block slices. The typed API generates independent `Bytes`
responses by copying; this allows answers to safely leave the core lock while the pages are
further modified.

<a id="abbild-api-und-dateirahmen"></a>

## Image API and file frame

```rust,no_run
use mynofy_server::message_image::{ImageLimits, MessageImage};
# fn example(engine: &mynofy_server::engine::Engine) -> Result<(), mynofy_server::BoxError> {
// Under the synchronization of the embedding owner, capture a consistent copy:
let image = engine.capture_message_image(ImageLimits::default())?;
// Then write outside the core lock; the caller determines file permissions and atomicity.
let mut options = std::fs::OpenOptions::new();
options.write(true).create_new(true);
#[cfg(unix)]
{
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}
let mut file = options.open("messages.bin")?;
image.write_to(&mut file)?;
file.sync_all()?;
let restored = MessageImage::read_from(std::fs::File::open("messages.bin")?, ImageLimits::default())?;
assert_eq!(image.message_count(), restored.message_count());
# Ok(()) }
```

Using new private files for product operation and renaming a synchronized temporary state
atomically when replaced; the generic writer does not handle file management. The API is a
library base, not a new admin command. `capture_message_image` copied under the owner
synchronization byte blocks and registry bindings. It is not constant work and requires
additional RAM. The returned image remains independent of subsequent Publishes/expirations; disk
I/O is done with its own image outside the lock.

The 96-byte file header contains `MYNIMG01`, format version 1, record width,
Slot-/table dimensions, registry numbers and record number. The first 64 bytes contain fields and
reserved zeros, the last 32 contain their SHA -256. Then follows owner ID, client and topic
bindings, content type table, record chunks and block pages in a set order. Each block has a
48-byte frame with order number u64, length u32, four reserved zero bytes and SHA -256 over the
payload bytes.

The registry bindings contain stable client IDs and topic names; they identify the numeric
references of the record and do not contain ACLs or keys; stringbytes are written directly as
UTF -8 blocks; Record-/Union-/Tabellen-/Seitenbytes correspond exactly to the RAM stock; a later
format version must not implicitly make the native Rust layout a file format.

`read_from` reads the blocks directly into the same RAM containers, checks before sharing and reconstructs their allocator lists and name index. This is not an unchecked memory mapping. Default limits: 64 MiB entire file, 100,000 resident records, 4096 chunk slots and 65,536 page slots; the caller can choose narrower or other matching limits. header and block lengths are limited before allocations.

Validated are Version/Breiten, checksums and block order, complete EOF without follow-up,
Bitmap/counter/Reserviertbytes, IDs and time values, registry limits, double IDs/Positionen,
Union types, parameters, all addresses and content type reference numbers. Shared content types
remain reference counted; unauthorized duplicate payload allocations and unreferenced occupied
blocks are rejected. There is no half-accepted database in the event of a read error.

<a id="umfang-und-einbindung"></a>

## Scope and involvement

`MessageImage` is a **message memory image**, not yet a complete owner checkpoint. It contains the resident messages including expiration times, payloads and registry bindings. Resident, logically expired records may be included; a future owner restore must continue to discard them before output after their unchanged absolute time. `message()` is used for explicit offline inspection, does not execute client authentication or topic authorization and is not a server read path.

The complete [Content-Checkpoint](content-storage.md) now embeds this message image together
with policies, headstands and assignment metadata. `MessageImage` remains the narrower
library interface; the service loads only the full checkpoint and only during the start.
Security status and content remain separate. A topic cursor recognized as invalid now
automatically starts at the active beginning; the ambiguity of a matching eight-digit value
remains.
