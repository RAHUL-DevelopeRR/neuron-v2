//! A size check of `.xls` workbooks before `calamine` opens them.
//!
//! `calamine` parses every sheet of an `.xls` file while opening it and lays
//! each one out as a dense grid from its first to its last cell, after
//! reserving room for the grid the sheet's DIMENSIONS record claims. Two
//! cells far apart, or one false claim, would ask for more memory than the
//! machine has, and a failed allocation aborts the program. This reads the
//! compound file's `Workbook` stream itself, walks the same records
//! `calamine` will, and refuses workbooks whose grids pass a limit.

const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// Sector ids at or above this are markers (free, end of chain, ...).
const FIRST_MARKER: u32 = 0xFFFF_FFFA;
const NO_STREAM: u32 = 0xFFFF_FFFF;
/// Sheets one workbook may list.
const MAX_SHEETS: usize = 4_096;

pub(super) fn is_compound_file(data: &[u8]) -> bool {
    data.starts_with(&SIGNATURE)
}

fn damaged(detail: &str) -> String {
    format!("this is not a readable spreadsheet: the file is damaged ({detail})")
}

/// Refuses `data` (an `.xls` file) when the grids `calamine` would build for
/// it hold more than `max_cells` cells in all.
pub(super) fn check(data: &[u8], max_cells: u64) -> Result<(), String> {
    let compound = Compound::open(data)?;
    let stream = compound.workbook_stream()?;
    check_workbook(&stream, max_cells)
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

    /// The `Workbook` (or older `Book`) stream.
    fn workbook_stream(&self) -> Result<Vec<u8>, String> {
        let directory = self.read(u32_at(self.data, 0x30).unwrap_or(NO_STREAM), None)?;
        let (entries, _) = directory.as_chunks::<128>();
        let size_of = |entry: &[u8]| -> usize {
            let low = u32_at(entry, 0x78).unwrap_or(0);
            let high = if self.sector_size == 512 {
                0
            } else {
                u32_at(entry, 0x7C).unwrap_or(0)
            };
            usize::try_from((u64::from(high) << 32) | u64::from(low)).unwrap_or(usize::MAX)
        };
        let name_of = |entry: &[u8]| -> String {
            let length = usize::from(u16_at(entry, 0x40).unwrap_or(0)).min(64);
            let units = (0..length.saturating_sub(2) / 2)
                .filter_map(|unit| u16_at(entry, unit * 2))
                .collect::<Vec<_>>();
            String::from_utf16_lossy(&units)
        };
        let workbook = ["Workbook", "Book"]
            .iter()
            .find_map(|wanted| {
                entries.iter().find(|entry| {
                    entry[0x42] == 2 && name_of(entry.as_slice()).eq_ignore_ascii_case(wanted)
                })
            })
            .ok_or_else(|| {
                "this is not a readable spreadsheet: the file is password protected or not an Excel workbook"
                    .to_string()
            })?;
        let start = u32_at(workbook, 0x74).unwrap_or(NO_STREAM);
        let size = size_of(workbook);
        let cutoff = index(u32_at(self.data, 0x38).unwrap_or(4096));
        if size >= cutoff {
            return self.read(start, Some(size));
        }
        // Small streams live in the mini stream, in 64-byte sectors.
        let root = entries
            .iter()
            .find(|entry| entry[0x42] == 5)
            .ok_or_else(|| damaged("the root entry is missing"))?;
        let mini_stream =
            self.read(u32_at(root, 0x74).unwrap_or(NO_STREAM), Some(size_of(root)))?;
        let mini_fat_bytes = self.read(u32_at(self.data, 0x3C).unwrap_or(NO_STREAM), None)?;
        let mini_fat = (0..mini_fat_bytes.len() / 4)
            .filter_map(|slot| u32_at(&mini_fat_bytes, slot * 4))
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        for id in Self::chain(start, &mini_fat)? {
            let begin = index(id)
                .checked_mul(64)
                .filter(|begin| *begin < mini_stream.len())
                .ok_or_else(|| damaged("a mini stream sector is missing"))?;
            out.extend_from_slice(&mini_stream[begin..(begin + 64).min(mini_stream.len())]);
            if out.len() >= size {
                break;
            }
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

fn check_workbook(stream: &[u8], max_cells: u64) -> Result<(), String> {
    // The globals substream lists each sheet (BoundSheet8) with the stream
    // offset of its records.
    let mut offsets = Vec::new();
    for (kind, data, _) in records(stream) {
        match kind {
            0x0085 => {
                if let Some(offset) = u32_at(data, 0) {
                    offsets.push(index(offset));
                }
            }
            0x000A => break,
            _ => {}
        }
    }
    if offsets.len() > MAX_SHEETS {
        return Err(damaged("it lists too many sheets"));
    }
    offsets.sort_unstable();
    let mut total = 0u64;
    let mut previous_end = 0usize;
    for offset in offsets {
        // calamine parses each listed sheet from its offset; sheets that
        // share records would multiply the work and memory.
        if offset < previous_end || offset > stream.len() {
            return Err(damaged("its sheets overlap"));
        }
        let (cells, length) = sheet_cells(&stream[offset..])?;
        previous_end = offset + length;
        total = total.saturating_add(cells);
        if total > max_cells {
            return Err(too_large(max_cells));
        }
    }
    Ok(())
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

/// The grid cells `calamine` reserves and builds for one sheet, and the
/// bytes of the sheet's records. Records are read as `calamine` reads them:
/// the sheet's own, not those of charts nested inside it.
fn sheet_cells(sheet: &[u8]) -> Result<(u64, usize), String> {
    let mut values = Bounds::default();
    let mut formulas = Bounds::default();
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
            }
            0x0006 if data.len() >= 20 => {
                values.add(row, column);
                formulas.add(row, column);
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
    Ok((cells, end))
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
