//! A size check of `.xls` workbooks before `calamine` opens them.
//!
//! `calamine` parses every sheet of an `.xls` file while opening it and lays
//! each one out as a dense grid from its first to its last cell, after
//! reserving room for the grid the sheet's DIMENSIONS record claims. Two
//! cells far apart, or one false claim, would ask for more memory than the
//! machine has, and a failed allocation aborts the program. This reads the
//! compound file's `Workbook` stream itself, picking it the way `calamine`'s
//! `cfb.rs` does, walks the same records `calamine` will, and refuses
//! workbooks whose grids pass a limit.
//!
//! Text is measured too: every cell that points at a shared string gets its
//! own copy of that string while `calamine` opens the file, so one string of
//! 65,535 characters named by many small records would ask for far more
//! memory than the file's size suggests. The shared string table is walked
//! for each string's length, and workbooks whose cells would copy more than
//! [`MAX_TEXT_BYTES`] of text are refused.

const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// Bytes of text `calamine` may copy into the cells of one workbook.
pub(super) const MAX_TEXT_BYTES: u64 = 256 * 1024 * 1024;
/// Sector ids at or above this are markers (free, end of chain, ...).
const FIRST_MARKER: u32 = 0xFFFF_FFFA;
const NO_STREAM: u32 = 0xFFFF_FFFF;
/// Streams shorter than this live in the mini stream (`calamine` uses this
/// fixed cutoff, not the header's).
const MINI_STREAM_CUTOFF: usize = 4096;
const MINI_SECTOR: usize = 64;
/// Sheets one workbook may list.
const MAX_SHEETS: usize = 4_096;

pub(super) fn is_compound_file(data: &[u8]) -> bool {
    data.starts_with(&SIGNATURE)
}

fn damaged(detail: &str) -> String {
    format!("this is not a readable spreadsheet: the file is damaged ({detail})")
}

/// Refuses `data` (an `.xls` file) when the grids `calamine` would build for
/// it hold more than `max_cells` cells in all, or its cells would copy more
/// than [`MAX_TEXT_BYTES`] of text.
pub(super) fn check(data: &[u8], max_cells: u64) -> Result<(), String> {
    check_with(data, max_cells, MAX_TEXT_BYTES)
}

/// [`check`] with a text limit of `max_text_bytes`.
pub(super) fn check_with(data: &[u8], max_cells: u64, max_text_bytes: u64) -> Result<(), String> {
    let compound = Compound::open(data)?;
    // `calamine` reads `Workbook` and, when that fails for any reason,
    // `Book`: both are checked when present.
    let streams = compound.workbook_streams()?;
    for stream in streams {
        check_workbook(&stream, max_cells, max_text_bytes)?;
    }
    Ok(())
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn index(id: u32) -> usize {
    usize::try_from(id).unwrap_or(usize::MAX)
}

/// An OLE compound file: just enough to read one stream out of it.
struct Compound<'a> {
    data: &'a [u8],
    sector_size: usize,
    /// The sector allocation table: the next sector of each sector's chain.
    fat: Vec<u32>,
}

impl<'a> Compound<'a> {
    fn open(data: &'a [u8]) -> Result<Self, String> {
        if !is_compound_file(data) || data.len() < 512 {
            return Err(damaged("no compound file header"));
        }
        let sector_size = match u16_at(data, 0x1E) {
            Some(9) => 512,
            Some(12) => 4096,
            _ => return Err(damaged("unknown sector size")),
        };
        let sectors = data.len().saturating_sub(sector_size).div_ceil(sector_size);
        let mut compound = Self {
            data,
            sector_size,
            fat: Vec::new(),
        };
        // The FAT's own sectors: 109 listed in the header, the rest in a
        // chain of DIFAT sectors.
        let mut fat_sectors = (0..109)
            .filter_map(|slot| u32_at(data, 0x4C + 4 * slot))
            .filter(|id| *id < FIRST_MARKER)
            .collect::<Vec<_>>();
        let mut next = u32_at(data, 0x44).unwrap_or(NO_STREAM);
        let mut hops = 0;
        while next < FIRST_MARKER {
            hops += 1;
            if hops > sectors {
                return Err(damaged("its allocation table loops"));
            }
            let sector = compound
                .sector(next)
                .ok_or_else(|| damaged("an allocation sector is missing"))?;
            let slots = sector.len() / 4;
            for slot in 0..slots.saturating_sub(1) {
                if let Some(id) = u32_at(sector, slot * 4).filter(|id| *id < FIRST_MARKER) {
                    fat_sectors.push(id);
                }
            }
            next = u32_at(sector, slots.saturating_sub(1) * 4).unwrap_or(NO_STREAM);
        }
        if fat_sectors.len() > sectors {
            return Err(damaged("its allocation table is larger than the file"));
        }
        let mut fat = Vec::new();
        for id in fat_sectors {
            let sector = compound
                .sector(id)
                .ok_or_else(|| damaged("an allocation sector is missing"))?;
            fat.extend((0..sector.len() / 4).filter_map(|slot| u32_at(sector, slot * 4)));
        }
        compound.fat = fat;
        Ok(compound)
    }

    fn sector(&self, id: u32) -> Option<&'a [u8]> {
        let start = index(id).checked_add(1)?.checked_mul(self.sector_size)?;
        if start >= self.data.len() {
            return None;
        }
        Some(&self.data[start..(start + self.sector_size).min(self.data.len())])
    }

    /// The sector ids of the chain starting at `start` in `table`.
    fn chain(start: u32, table: &[u32]) -> Result<Vec<u32>, String> {
        let mut chain = Vec::new();
        let mut next = start;
        while next < FIRST_MARKER {
            if chain.len() >= table.len() {
                return Err(damaged("a sector chain loops"));
            }
            chain.push(next);
            next = *table
                .get(index(next))
                .ok_or_else(|| damaged("a sector chain leaves the file"))?;
        }
        Ok(chain)
    }

    /// `size` bytes of the stream starting at sector `start`; all of the
    /// chain when `size` is `None`.
    fn read(&self, start: u32, size: Option<usize>) -> Result<Vec<u8>, String> {
        if size.is_some_and(|size| size > self.data.len()) {
            return Err(damaged("a stream is larger than the file"));
        }
        let mut out = Vec::new();
        for id in Self::chain(start, &self.fat)? {
            if size.is_some_and(|size| out.len() >= size) {
                break;
            }
            out.extend_from_slice(
                self.sector(id)
                    .ok_or_else(|| damaged("a stream sector is missing"))?,
            );
        }
        if let Some(size) = size {
            if out.len() < size {
                return Err(damaged("a stream is cut short"));
            }
            out.truncate(size);
        }
        Ok(out)
    }

    /// The `Workbook` and older `Book` streams that are present, found as
    /// `calamine` finds them: the directory read as a flat list in file
    /// order, each name decoded from all 64 name bytes and cut at the first
    /// NUL, the first entry of a matching name taken whatever its type.
    fn workbook_streams(&self) -> Result<Vec<Vec<u8>>, String> {
        let directory = self.read(u32_at(self.data, 0x30).unwrap_or(NO_STREAM), None)?;
        let (entries, _) = directory.as_chunks::<128>();
        let name_of = |entry: &[u8; 128]| -> String {
            let units = (0..32)
                .filter_map(|unit| u16_at(entry, unit * 2))
                .collect::<Vec<_>>();
            let mut name = String::from_utf16_lossy(&units);
            if let Some(end) = name.find('\0') {
                name.truncate(end);
            }
            name
        };
        let found = ["Workbook", "Book"]
            .iter()
            .filter_map(|wanted| {
                entries
                    .iter()
                    .find(|entry| name_of(entry).eq_ignore_ascii_case(wanted))
            })
            .collect::<Vec<_>>();
        if found.is_empty() {
            return Err(
                "this is not a readable spreadsheet: the file is password protected or not an Excel workbook"
                    .to_string(),
            );
        }
        found
            .into_iter()
            .map(|entry| self.entry_stream(entry, entries))
            .collect()
    }

    /// The size a directory entry gives its stream.
    fn size_of(&self, entry: &[u8]) -> usize {
        let low = u32_at(entry, 0x78).unwrap_or(0);
        let high = if self.sector_size == 512 {
            0
        } else {
            u32_at(entry, 0x7C).unwrap_or(0)
        };
        usize::try_from((u64::from(high) << 32) | u64::from(low)).unwrap_or(usize::MAX)
    }

    /// The bytes of the stream `entry` names. A small stream is read from
    /// the mini stream of the first directory entry, as `calamine` does; one
    /// that would need bytes past that mini stream (which `calamine` would
    /// take from elsewhere in the file) is refused.
    fn entry_stream(&self, entry: &[u8], entries: &[[u8; 128]]) -> Result<Vec<u8>, String> {
        let start = u32_at(entry, 0x74).unwrap_or(NO_STREAM);
        let size = self.size_of(entry);
        if size >= MINI_STREAM_CUTOFF {
            return self.read(start, Some(size));
        }
        let mini_fat_sectors = index(u32_at(self.data, 0x40).unwrap_or(0));
        let root = entries
            .first()
            .ok_or_else(|| damaged("the root entry is missing"))?;
        if mini_fat_sectors == 0 {
            return Err(damaged("a small stream has no mini stream"));
        }
        let mini_stream = self.read(
            u32_at(root, 0x74).unwrap_or(NO_STREAM),
            Some(self.size_of(root)),
        )?;
        let mini_fat_bytes = self.read(
            u32_at(self.data, 0x3C).unwrap_or(NO_STREAM),
            Some(mini_fat_sectors.saturating_mul(self.sector_size)),
        )?;
        let mini_fat = (0..mini_fat_bytes.len() / 4)
            .filter_map(|slot| u32_at(&mini_fat_bytes, slot * 4))
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        for id in Self::chain(start, &mini_fat)? {
            if out.len() >= size {
                break;
            }
            let begin = index(id)
                .checked_mul(MINI_SECTOR)
                .filter(|begin| begin + MINI_SECTOR <= mini_stream.len())
                .ok_or_else(|| damaged("a mini stream sector is missing"))?;
            out.extend_from_slice(&mini_stream[begin..begin + MINI_SECTOR]);
        }
        if out.len() < size {
            return Err(damaged("a stream is cut short"));
        }
        out.truncate(size);
        Ok(out)
    }
}

/// BIFF records from `stream`: type and data.
fn records(stream: &[u8]) -> impl Iterator<Item = (u16, &[u8], usize)> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        let kind = u16_at(stream, pos)?;
        let length = usize::from(u16_at(stream, pos + 2)?);
        let data = stream.get(pos + 4..pos + 4 + length)?;
        pos += 4 + length;
        Some((kind, data, pos))
    })
}

fn too_large(max_cells: u64) -> String {
    format!(
        "this .xls workbook lays its sheets out over more than {max_cells} cells, more than read_document reads; save it as .xlsx and read that instead"
    )
}

fn too_much_text(max_text_bytes: u64) -> String {
    format!(
        "this .xls workbook copies more than {} MB of text into its cells, more than read_document reads; save it as .xlsx and read that instead",
        max_text_bytes / (1024 * 1024)
    )
}

fn check_workbook(stream: &[u8], max_cells: u64, max_text_bytes: u64) -> Result<(), String> {
    // The globals substream lists each sheet (BoundSheet8) with the stream
    // offset of its records, and holds the shared string table.
    let mut offsets = Vec::new();
    let mut strings = Vec::new();
    // UTF-8 bytes one character of a string may become: up to 3 from UTF-16
    // (the BIFF8 code page); another code page reads each of the 2 bytes
    // `calamine` makes of a character as one character.
    let mut bytes_per_char = 3u64;
    for (kind, data, after) in records(stream) {
        match kind {
            0x0085 => {
                if let Some(offset) = u32_at(data, 0) {
                    offsets.push(index(offset));
                }
            }
            0x0042 => {
                if !matches!(u16_at(data, 0), Some(1200 | 1201)) {
                    bytes_per_char = 6;
                }
            }
            // SST: calamine keeps the last one.
            0x00FC => strings = shared_strings(data, &continues(stream, after))?,
            0x000A => break,
            _ => {}
        }
    }
    if offsets.len() > MAX_SHEETS {
        return Err(damaged("it lists too many sheets"));
    }
    offsets.sort_unstable();
    let mut total = 0u64;
    let mut text = 0u64;
    let mut previous_end = 0usize;
    for offset in offsets {
        // calamine parses each listed sheet from its offset; sheets that
        // share records would multiply the work and memory.
        if offset < previous_end || offset > stream.len() {
            return Err(damaged("its sheets overlap"));
        }
        let sheet = sheet_cells(&stream[offset..], &strings)?;
        previous_end = offset + sheet.length;
        total = total.saturating_add(sheet.cells);
        if total > max_cells {
            return Err(too_large(max_cells));
        }
        text = text.saturating_add(sheet.chars.saturating_mul(bytes_per_char));
        if text > max_text_bytes {
            return Err(too_much_text(max_text_bytes));
        }
    }
    Ok(())
}

/// The CONTINUE records right after the record that ends at `at`, as
/// `calamine` joins them to it.
fn continues(stream: &[u8], mut at: usize) -> Vec<&[u8]> {
    let mut found = Vec::new();
    while stream.len().saturating_sub(at) > 4 && u16_at(stream, at) == Some(0x003C) {
        let Some(length) = u16_at(stream, at + 2).map(usize::from) else {
            break;
        };
        let Some(data) = stream.get(at + 4..at + 4 + length) else {
            break;
        };
        found.push(data);
        at += 4 + length;
    }
    found
}

/// A record and the CONTINUE records after it, read as one, the way
/// `calamine` reads a shared string table.
struct Continued<'a> {
    data: &'a [u8],
    rest: std::slice::Iter<'a, &'a [u8]>,
}

impl Continued<'_> {
    /// Moves on to the next CONTINUE record; false when there is none.
    fn next_part(&mut self) -> bool {
        match self.rest.next() {
            Some(data) => {
                self.data = data;
                true
            }
            None => false,
        }
    }

    fn skip(&mut self, mut count: usize) -> Result<(), String> {
        while count > 0 {
            if self.data.is_empty() && !self.next_part() {
                return Err(sst_damaged());
            }
            let step = count.min(self.data.len());
            self.data = &self.data[step..];
            count -= step;
        }
        Ok(())
    }
}

fn sst_damaged() -> String {
    damaged("its shared string table is cut short")
}

/// The length in characters of each string in a shared string table (SST)
/// whose record holds `data`, followed by `continues`, read as `calamine`
/// reads it. A table `calamine` would fail on is refused here, which reads
/// the same as calamine's own failure would.
fn shared_strings(data: &[u8], continues: &[&[u8]]) -> Result<Vec<u16>, String> {
    let mut record = Continued {
        data: data.get(8..).ok_or_else(sst_damaged)?,
        rest: continues.iter(),
    };
    let mut strings = Vec::new();
    while !record.data.is_empty() || record.next_part() {
        // XLUnicodeRichExtendedString: an empty part is an empty string.
        if record.data.is_empty() {
            strings.push(0);
            continue;
        }
        let (Some(count), Some(&flags)) = (u16_at(record.data, 0), record.data.get(2)) else {
            return Err(sst_damaged());
        };
        record.data = &record.data[3..];
        let mut runs = 0usize;
        if flags & 0x8 != 0 {
            runs = usize::from(u16_at(record.data, 0).ok_or_else(sst_damaged)?);
            record.data = &record.data[2..];
        }
        let mut extra = 0usize;
        if flags & 0x4 != 0 {
            let size = u32_at(record.data, 0).ok_or_else(sst_damaged)?;
            // calamine reads it as signed: a negative size skips past the end.
            extra = usize::try_from(i32::from_le_bytes(size.to_le_bytes())).unwrap_or(usize::MAX);
            record.data = &record.data[4..];
        }
        // The characters, one or two bytes each, running on into the next
        // CONTINUE record (which starts with its own width flag).
        let mut left = usize::from(count);
        let mut wide = flags & 0x1 != 0;
        while left > 0 {
            let width = if wide { 2 } else { 1 };
            let taken = (record.data.len() / width).min(left);
            record.data = &record.data[taken * width..];
            left -= taken;
            if left > 0 {
                if !record.next_part() {
                    return Err(sst_damaged());
                }
                let Some(&part_flags) = record.data.first() else {
                    return Err(sst_damaged());
                };
                wide = part_flags & 0x1 != 0;
                record.data = &record.data[1..];
            }
        }
        record.skip(runs.saturating_mul(4))?;
        record.skip(extra)?;
        strings.push(count);
    }
    Ok(strings)
}

/// Characters of an `XLUnicodeString` (a length, then the text) at the start
/// of `data`: no more than the record holds, as `calamine` decodes only that.
fn string_chars(data: Option<&[u8]>) -> u64 {
    data.and_then(|data| {
        let count = usize::from(u16_at(data, 0)?);
        u64::try_from(count.min(data.len())).ok()
    })
    .unwrap_or(0)
}

/// The smallest box holding a set of cells.
#[derive(Default)]
struct Bounds(Option<(u32, u32, u32, u32)>);

impl Bounds {
    fn add(&mut self, row: u32, column: u32) {
        self.0 = Some(match self.0 {
            None => (row, row, column, column),
            Some((top, bottom, left, right)) => (
                top.min(row),
                bottom.max(row),
                left.min(column),
                right.max(column),
            ),
        });
    }

    fn cells(&self) -> u64 {
        self.0.map_or(0, |(top, bottom, left, right)| {
            u64::from(bottom - top + 1) * u64::from(right - left + 1)
        })
    }
}

/// What `calamine` builds for one sheet.
struct SheetSize {
    /// Grid cells it reserves and builds.
    cells: u64,
    /// Characters of text it copies into the cells.
    chars: u64,
    /// Bytes of the sheet's records.
    length: usize,
}

/// What `calamine` builds for one sheet whose shared strings are `strings`
/// characters long. Records are read as `calamine` reads them: the sheet's
/// own, not those of charts nested inside it.
fn sheet_cells(sheet: &[u8], strings: &[u16]) -> Result<SheetSize, String> {
    let mut chars = 0u64;
    let mut values = Bounds::default();
    let mut formulas = Bounds::default();
    // Where the last FORMULA record was: `calamine` places each STRING
    // record (a formula's text result) there, at A1 before any formula.
    let mut formula_at = (0u32, 0u32);
    let mut reserved = 0u64;
    let mut seen_sheet_start = false;
    let mut nested = 0usize;
    let mut end = sheet.len();
    for (kind, data, after) in records(sheet) {
        if nested > 0 {
            match kind {
                0x0809 => nested += 1,
                0x000A => nested -= 1,
                _ => {}
            }
            continue;
        }
        let row = u32::from(u16_at(data, 0).unwrap_or(0));
        let column = u32::from(u16_at(data, 2).unwrap_or(0));
        match kind {
            0x0809 => {
                if seen_sheet_start {
                    nested += 1;
                } else {
                    seen_sheet_start = true;
                }
            }
            // DIMENSIONS: calamine reserves rows x columns from this claim.
            0x0200 => reserved = reserved.max(dimensions(data)?),
            0x0203 | 0x0204 | 0x00D6 | 0x0205 | 0x027E | 0x00FD if data.len() >= 4 => {
                values.add(row, column);
                match kind {
                    // LABEL, RSTRING: the text is in the record.
                    0x0204 | 0x00D6 => chars = chars.saturating_add(string_chars(data.get(6..))),
                    // LABELSST: a copy of a shared string, whatever its length.
                    0x00FD if data.len() >= 10 => {
                        let shared = u32_at(data, 6)
                            .and_then(|at| strings.get(index(at)))
                            .copied()
                            .unwrap_or(0);
                        chars = chars.saturating_add(u64::from(shared));
                    }
                    _ => {}
                }
            }
            0x0006 if data.len() >= 20 => {
                formula_at = (row, column);
                values.add(row, column);
                formulas.add(row, column);
            }
            // STRING: a cell at the last formula's position, whatever its
            // length (a short one only makes calamine fail, which is safe).
            0x0207 => {
                values.add(formula_at.0, formula_at.1);
                chars = chars.saturating_add(string_chars(Some(data)));
            }
            // MULRK: one row, a run of columns.
            0x00BD if data.len() >= 6 => {
                let last = u32::from(u16_at(data, data.len() - 2).unwrap_or(0));
                if last >= column {
                    values.add(row, column);
                    values.add(row, last);
                }
            }
            0x000A => {
                end = after;
                break;
            }
            _ => {}
        }
    }
    let cells = reserved
        .saturating_add(values.cells())
        .saturating_add(formulas.cells());
    Ok(SheetSize {
        cells,
        chars,
        length: end,
    })
}

/// The cells `calamine` reserves for a DIMENSIONS record, mirroring its
/// arithmetic; a claim its arithmetic would wrap into a huge number is
/// refused.
fn dimensions(data: &[u8]) -> Result<u64, String> {
    let (first_row, last_row, mut first_column, last_column) = match data.len() {
        10 => (
            u32::from(u16_at(data, 0).unwrap_or(0)),
            u32::from(u16_at(data, 2).unwrap_or(0)),
            u32::from(u16_at(data, 4).unwrap_or(0)),
            u32::from(u16_at(data, 6).unwrap_or(0)),
        ),
        14 | 16 => (
            u32_at(data, 0).unwrap_or(0),
            u32_at(data, 4).unwrap_or(0),
            u32::from(u16_at(data, 8).unwrap_or(0)),
            u32::from(u16_at(data, 10).unwrap_or(0)),
        ),
        // calamine rejects other lengths without reserving anything.
        _ => return Ok(0),
    };
    if first_column > 0xFF || last_column < first_column {
        first_column = 0;
    }
    if last_row == 0 || last_column == 0 {
        return Ok(1);
    }
    if last_row < first_row {
        return Err(damaged("a sheet's size record is invalid"));
    }
    Ok(u64::from(last_row - first_row) * u64::from(last_column - first_column))
}
