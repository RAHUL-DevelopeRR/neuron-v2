//! Line diffs for the full-screen UI's change views: how many lines a file gained and
//! lost, and its unified-diff hunks (from the first `@@` on, as `git diff` writes them),
//! for the files git cannot diff for us: untracked files, and the agent's changes
//! outside a repository.
//!
//! The edit script is Myers' shortest one, after the lines both versions start and end
//! with are set aside. Past [`MAX_EDIT_DISTANCE`] edits it stops looking and replaces the
//! whole middle, so a rewritten file costs no more than a small edit.

use std::collections::HashMap;
use std::fmt::Write as _;

/// Unchanged lines shown around each change, as `git diff` does.
const CONTEXT_LINES: usize = 3;
/// Edits the shortest-script search looks for before it replaces the middle instead.
const MAX_EDIT_DISTANCE: usize = 1_000;
/// Bytes looked at for a NUL, as git does to call a file binary.
const BINARY_SNIFF_BYTES: usize = 8_000;

/// Lines added and removed between two versions of a file, and the hunks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LineDiff {
    pub(crate) added: u64,
    pub(crate) removed: u64,
    /// Unified-diff hunks; empty when nothing changed.
    pub(crate) patch: String,
}

/// Whether `bytes` look binary to git: a NUL near the start.
pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0)
}

/// Line breaks in `bytes`.
pub(crate) fn line_breaks(bytes: &[u8]) -> usize {
    bytes.split(|byte| *byte == b'\n').count() - 1
}

/// Lines in `bytes`, counting a last line without a line break.
pub(crate) fn count_lines(bytes: &[u8]) -> u64 {
    let breaks = line_breaks(bytes);
    let unterminated = usize::from(bytes.last().is_some_and(|byte| *byte != b'\n'));
    u64::try_from(breaks + unterminated).unwrap_or(u64::MAX)
}

/// The changes from `old` to `new`, line by line. A changed line ending counts as a
/// changed line, as it does for git.
pub(crate) fn diff_lines(old: &str, new: &str) -> LineDiff {
    let old_lines = old.split_inclusive('\n').collect::<Vec<_>>();
    let new_lines = new.split_inclusive('\n').collect::<Vec<_>>();
    let ops = edit_script(&old_lines, &new_lines);
    let count = |wanted: Op| u64::try_from(ops.iter().filter(|op| **op == wanted).count());
    LineDiff {
        added: count(Op::Insert).unwrap_or(u64::MAX),
        removed: count(Op::Delete).unwrap_or(u64::MAX),
        patch: hunks(&old_lines, &new_lines, &ops),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Equal,
    Delete,
    Insert,
}

fn edit_script(old: &[&str], new: &[&str]) -> Vec<Op> {
    let prefix = old
        .iter()
        .zip(new)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let old_middle = &old[prefix..old.len() - suffix];
    let new_middle = &new[prefix..new.len() - suffix];
    let mut ops = vec![Op::Equal; prefix];
    let (old_ids, new_ids) = line_ids(old_middle, new_middle);
    match shortest_edit(&old_ids, &new_ids) {
        Some(middle) => ops.extend(middle),
        None => {
            ops.extend(std::iter::repeat_n(Op::Delete, old_middle.len()));
            ops.extend(std::iter::repeat_n(Op::Insert, new_middle.len()));
        }
    }
    ops.extend(std::iter::repeat_n(Op::Equal, suffix));
    ops
}

/// Each distinct line as a number, so the search compares numbers, not text.
fn line_ids<'a>(old: &[&'a str], new: &[&'a str]) -> (Vec<u32>, Vec<u32>) {
    let mut ids: HashMap<&'a str, u32> = HashMap::new();
    let mut id_of = |line: &'a str| {
        let next = u32::try_from(ids.len()).unwrap_or(u32::MAX);
        *ids.entry(line).or_insert(next)
    };
    let old = old.iter().map(|line| id_of(line)).collect();
    let new = new.iter().map(|line| id_of(line)).collect();
    (old, new)
}

/// Myers' shortest edit script from `old` to `new`, or `None` when it needs more than
/// [`MAX_EDIT_DISTANCE`] edits. `trace[d]` keeps, for every diagonal `k` in `-d..=d`
/// (at index `k + d`), the furthest `x` reached with `d` edits.
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn shortest_edit(old: &[u32], new: &[u32]) -> Option<Vec<Op>> {
    // Lengths are line counts of files of a few megabytes at most: they fit an isize.
    let (n, m) = (old.len() as isize, new.len() as isize);
    let limit = (n + m).min(MAX_EDIT_DISTANCE as isize);
    let offset = limit + 1;
    let mut furthest = vec![0_isize; (2 * limit + 3) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=limit {
        for k in (-d..=d).step_by(2) {
            let at = (k + offset) as usize;
            let mut x = if k == -d || (k != d && furthest[at - 1] < furthest[at + 1]) {
                furthest[at + 1]
            } else {
                furthest[at - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && old[x as usize] == new[y as usize] {
                x += 1;
                y += 1;
            }
            furthest[at] = x;
            if x >= n && y >= m {
                trace.push(furthest[(offset - d) as usize..=(offset + d) as usize].to_vec());
                return Some(walk_back(&trace, n, m));
            }
        }
        trace.push(furthest[(offset - d) as usize..=(offset + d) as usize].to_vec());
    }
    None
}

/// The edit script, read back from the end of [`shortest_edit`]'s search.
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn walk_back(trace: &[Vec<isize>], n: isize, m: isize) -> Vec<Op> {
    let mut ops = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..trace.len() as isize).rev() {
        let before = &trace[(d - 1) as usize];
        let reached = |k: isize| before[(k + d - 1) as usize];
        let k = x - y;
        let from_k = if k == -d || (k != d && reached(k - 1) < reached(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let from_x = reached(from_k);
        let from_y = from_x - from_k;
        while x > from_x && y > from_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        ops.push(if x == from_x { Op::Insert } else { Op::Delete });
        x = from_x;
        y = from_y;
    }
    while x > 0 && y > 0 {
        ops.push(Op::Equal);
        x -= 1;
        y -= 1;
    }
    ops.reverse();
    ops
}

/// The script as unified-diff hunks with [`CONTEXT_LINES`] of context.
fn hunks(old: &[&str], new: &[&str], ops: &[Op]) -> String {
    // Where each op starts in the old and new file.
    let mut positions = Vec::with_capacity(ops.len());
    let (mut old_at, mut new_at) = (0, 0);
    for op in ops {
        positions.push((old_at, new_at));
        match op {
            Op::Equal => {
                old_at += 1;
                new_at += 1;
            }
            Op::Delete => old_at += 1,
            Op::Insert => new_at += 1,
        }
    }
    let changes = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| **op != Op::Equal)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut patch = String::new();
    let mut next = 0;
    while next < changes.len() {
        let first = changes[next];
        let mut last = first;
        next += 1;
        while next < changes.len() && changes[next] - last <= 2 * CONTEXT_LINES + 1 {
            last = changes[next];
            next += 1;
        }
        let start = first.saturating_sub(CONTEXT_LINES);
        let end = (last + CONTEXT_LINES + 1).min(ops.len());
        let span = &ops[start..end];
        let old_len = span.iter().filter(|op| **op != Op::Insert).count();
        let new_len = span.iter().filter(|op| **op != Op::Delete).count();
        let (old_start, new_start) = positions[start];
        let _ = writeln!(
            patch,
            "@@ -{} +{} @@",
            hunk_range(old_start, old_len),
            hunk_range(new_start, new_len)
        );
        for (op, (old_at, new_at)) in span.iter().zip(&positions[start..end]) {
            let (marker, line) = match op {
                Op::Equal => (' ', old[*old_at]),
                Op::Delete => ('-', old[*old_at]),
                Op::Insert => ('+', new[*new_at]),
            };
            patch.push(marker);
            match line.strip_suffix('\n') {
                Some(text) => {
                    patch.push_str(text.strip_suffix('\r').unwrap_or(text));
                    patch.push('\n');
                }
                None => {
                    patch.push_str(line);
                    patch.push_str("\n\\ No newline at end of file\n");
                }
            }
        }
    }
    patch
}

/// `start,len` as a hunk header writes it: 1-based, the line before for an empty range,
/// and no `,1`.
fn hunk_range(start: usize, len: usize) -> String {
    match len {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        _ => format!("{},{len}", start + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::{count_lines, diff_lines, is_binary, LineDiff, MAX_EDIT_DISTANCE};
    use std::fmt::Write as _;

    /// One line per number: `{prefix}{number}`.
    fn numbered(prefix: &str, numbers: impl Iterator<Item = usize>) -> String {
        numbers.fold(String::new(), |mut text, number| {
            let _ = writeln!(text, "{prefix}{number}");
            text
        })
    }

    #[test]
    fn an_edit_in_the_middle_shows_with_three_lines_of_context() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let new = "1\n2\n3\n4\nfive\n6\n7\n8\n9\n";
        assert_eq!(
            diff_lines(old, new),
            LineDiff {
                added: 1,
                removed: 1,
                patch: "@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-5\n+five\n 6\n 7\n 8\n".to_string(),
            }
        );
    }

    #[test]
    fn distant_changes_get_hunks_of_their_own_and_close_ones_share_one() {
        let old = numbered("", 1..=30);
        let new = (1..=30)
            .filter(|line| *line != 25)
            .map(|line| match line {
                2 => "two\n".to_string(),
                5 => "five\n".to_string(),
                other => format!("{other}\n"),
            })
            .collect::<String>();
        let diff = diff_lines(&old, &new);
        assert_eq!((diff.added, diff.removed), (2, 3));
        let headers = diff
            .patch
            .lines()
            .filter(|line| line.starts_with("@@"))
            .collect::<Vec<_>>();
        assert_eq!(headers, ["@@ -1,8 +1,8 @@", "@@ -22,7 +22,6 @@"]);
    }

    #[test]
    fn new_and_removed_files_are_all_additions_or_all_removals() {
        assert_eq!(
            diff_lines("", "a\nb\n"),
            LineDiff {
                added: 2,
                removed: 0,
                patch: "@@ -0,0 +1,2 @@\n+a\n+b\n".to_string(),
            }
        );
        let removed = diff_lines("only\n", "");
        assert_eq!(removed.patch, "@@ -1 +0,0 @@\n-only\n");
        assert_eq!(diff_lines("same\n", "same\n"), LineDiff::default());
    }

    #[test]
    fn a_last_line_without_a_break_is_marked_and_line_endings_count() {
        let diff = diff_lines("a\nb", "a\nb\n");
        assert_eq!((diff.added, diff.removed), (1, 1));
        assert_eq!(
            diff.patch,
            "@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n"
        );
        let crlf = diff_lines("a\r\nb\r\n", "a\nb\r\n");
        assert_eq!((crlf.added, crlf.removed), (1, 1));
        assert!(!crlf.patch.contains('\r'), "{:?}", crlf.patch);
    }

    #[test]
    fn a_rewrite_past_the_search_limit_replaces_the_middle() {
        let old = numbered("old ", 0..MAX_EDIT_DISTANCE * 2);
        let new = numbered("new ", 0..MAX_EDIT_DISTANCE * 2);
        let diff = diff_lines(&format!("head\n{old}tail\n"), &format!("head\n{new}tail\n"));
        let lines = u64::try_from(MAX_EDIT_DISTANCE * 2).expect("fits");
        assert_eq!((diff.added, diff.removed), (lines, lines));
        let header = format!("@@ -1,{0} +1,{0} @@\n head\n-old 0\n", lines + 2);
        assert!(diff.patch.starts_with(&header), "{}", &diff.patch[..80]);
    }

    #[test]
    fn the_shortest_script_keeps_moved_and_repeated_lines_minimal() {
        let diff = diff_lines("a\nb\nc\na\nb\nb\na\n", "c\nb\na\nb\na\nc\n");
        // Myers' classic example: five edits.
        assert_eq!(diff.added + diff.removed, 5);
    }

    #[test]
    fn line_counts_and_binary_detection() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"one"), 1);
        assert_eq!(count_lines(b"one\ntwo\n"), 2);
        assert!(is_binary(b"PNG\0\x01"));
        assert!(!is_binary("plain text, ünïcode".as_bytes()));
    }
}
