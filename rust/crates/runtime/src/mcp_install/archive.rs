//! Reading the archives verified installs unpack: gzipped tar (npm tarballs, release
//! `.tar.gz`) and zip (release `.zip`). Only regular files and folders are written;
//! links, devices and any path that would leave the destination are skipped, so an
//! archive cannot write outside the folder it is unpacked into.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::mcp_catalog::relative_member;

const BLOCK: usize = 512;

/// One file of a tar archive, after its name was resolved.
struct TarFile {
    name: String,
    size: u64,
    kind: u8,
    mode: u32,
}

fn octal(field: &[u8]) -> io::Result<u64> {
    let text = field
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| char::from(*byte))
        .collect::<String>();
    let text = text.trim();
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad number in a tar header"))
}

fn header_text(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// The `path=` record of a pax extended header, if it has one.
fn pax_path(data: &[u8]) -> Option<String> {
    let mut rest = data;
    while !rest.is_empty() {
        let space = rest.iter().position(|byte| *byte == b' ')?;
        let length: usize = std::str::from_utf8(&rest[..space]).ok()?.parse().ok()?;
        if length <= space || length > rest.len() {
            return None;
        }
        let record = &rest[space + 1..length];
        let record = record.strip_suffix(b"\n").unwrap_or(record);
        if let Some(path) = record.strip_prefix(b"path=") {
            return Some(String::from_utf8_lossy(path).into_owned());
        }
        rest = &rest[length..];
    }
    None
}

fn read_block(reader: &mut impl Read, block: &mut [u8; BLOCK]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < BLOCK {
        let read = reader.read(&mut block[filled..])?;
        if read == 0 {
            return if filled == 0 {
                Ok(false)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the tar archive ends inside a header",
                ))
            };
        }
        filled += read;
    }
    Ok(true)
}

fn read_exact_vec(reader: &mut impl Read, size: u64) -> io::Result<Vec<u8>> {
    if size > 1 << 20 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a tar extended header is larger than 1 MiB",
        ));
    }
    let mut data = vec![0; usize::try_from(size).unwrap_or(0)];
    reader.read_exact(&mut data)?;
    skip_padding(reader, size)?;
    Ok(data)
}

fn skip_padding(reader: &mut impl Read, size: u64) -> io::Result<()> {
    let padding = (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64;
    io::copy(&mut reader.take(padding), &mut io::sink())?;
    Ok(())
}

/// The next file entry of a tar stream, with GNU long names and pax paths applied.
fn next_tar_file(reader: &mut impl Read) -> io::Result<Option<TarFile>> {
    let mut long_name: Option<String> = None;
    let mut block = [0u8; BLOCK];
    loop {
        if !read_block(reader, &mut block)? {
            return Ok(None);
        }
        if block.iter().all(|byte| *byte == 0) {
            return Ok(None);
        }
        let size = octal(&block[124..136])?;
        let kind = block[156];
        match kind {
            b'L' => {
                let data = read_exact_vec(reader, size)?;
                long_name = Some(header_text(&data));
                continue;
            }
            b'x' => {
                let data = read_exact_vec(reader, size)?;
                if let Some(path) = pax_path(&data) {
                    long_name = Some(path);
                }
                continue;
            }
            b'g' => {
                read_exact_vec(reader, size)?;
                continue;
            }
            _ => {}
        }
        let name = long_name.take().unwrap_or_else(|| {
            let name = header_text(&block[0..100]);
            let prefix = if &block[257..262] == b"ustar" {
                header_text(&block[345..500])
            } else {
                String::new()
            };
            if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            }
        });
        let mode = u32::try_from(octal(&block[100..108])? & 0o7777).unwrap_or(0o644);
        return Ok(Some(TarFile {
            name,
            size,
            kind,
            mode,
        }));
    }
}

/// A tar member's name as a path under the destination: `./` and trailing `/` dropped,
/// the first `strip` parts removed, and `None` when nothing is left or it would leave
/// the destination.
fn member_path(name: &str, strip: usize) -> Option<String> {
    let name = name.replace('\\', "/");
    let parts = name
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>();
    if parts.len() <= strip {
        return None;
    }
    let path = parts[strip..].join("/");
    relative_member(&path).then_some(path)
}

/// Unpacks the gzipped tar at `archive` into `dest`, dropping the first `strip` parts of
/// every name (npm tarballs keep their files under `package/`). Returns the files
/// written, relative to `dest`.
pub(super) fn unpack_tar_gz(archive: &Path, dest: &Path, strip: usize) -> io::Result<Vec<String>> {
    let mut reader = flate2::read::GzDecoder::new(io::BufReader::new(fs::File::open(archive)?));
    let mut written = Vec::new();
    while let Some(file) = next_tar_file(&mut reader)? {
        let target = member_path(&file.name, strip);
        match (file.kind, target) {
            (b'0' | 0 | b'7', Some(path)) => {
                let out = dest.join(&path);
                if let Some(parent) = out.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut output = fs::File::create(&out)?;
                let copied = io::copy(&mut (&mut reader).take(file.size), &mut output)?;
                if copied != file.size {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!("the tar archive ends inside {path}"),
                    ));
                }
                output.flush()?;
                set_mode(&out, file.mode)?;
                skip_padding(&mut reader, file.size)?;
                written.push(path);
            }
            (b'5', Some(path)) => {
                fs::create_dir_all(dest.join(path))?;
                io::copy(&mut (&mut reader).take(file.size), &mut io::sink())?;
                skip_padding(&mut reader, file.size)?;
            }
            _ => {
                // Links, devices, and names that leave the folder are not written.
                io::copy(&mut (&mut reader).take(file.size), &mut io::sink())?;
                skip_padding(&mut reader, file.size)?;
            }
        }
    }
    Ok(written)
}

/// Writes the tar member `member` of the gzipped tar at `archive` to `out`.
pub(super) fn extract_tar_gz_member(archive: &Path, member: &str, out: &Path) -> io::Result<()> {
    let mut reader = flate2::read::GzDecoder::new(io::BufReader::new(fs::File::open(archive)?));
    while let Some(file) = next_tar_file(&mut reader)? {
        let regular = matches!(file.kind, b'0' | 0 | b'7');
        if regular && member_path(&file.name, 0).as_deref() == Some(member) {
            let mut output = fs::File::create(out)?;
            let copied = io::copy(&mut (&mut reader).take(file.size), &mut output)?;
            if copied != file.size {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("the tar archive ends inside {member}"),
                ));
            }
            return output.flush();
        }
        io::copy(&mut (&mut reader).take(file.size), &mut io::sink())?;
        skip_padding(&mut reader, file.size)?;
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("the archive has no {member}"),
    ))
}

fn le16(bytes: &[u8], at: usize) -> u64 {
    u64::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
}

fn le32(bytes: &[u8], at: usize) -> u64 {
    u64::from(u32::from_le_bytes([
        bytes[at],
        bytes[at + 1],
        bytes[at + 2],
        bytes[at + 3],
    ]))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// Writes the zip member `member` (stored or deflated) of the archive at `archive` to
/// `out`. Zip64 archives are not read.
pub(super) fn extract_zip_member(archive: &Path, member: &str, out: &Path) -> io::Result<()> {
    let mut file = fs::File::open(archive)?;
    let length = file.metadata()?.len();
    // The end-of-central-directory record is in the last 64 KiB plus its 22 bytes.
    let tail_length = length.min(65_557);
    file.seek(SeekFrom::Start(length - tail_length))?;
    let mut tail = vec![0; usize::try_from(tail_length).unwrap_or(0)];
    file.read_exact(&mut tail)?;
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|at| tail[*at..*at + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(|| invalid("not a zip archive"))?;
    let count = le16(&tail, end + 10);
    let directory_size = le32(&tail, end + 12);
    let directory_offset = le32(&tail, end + 16);
    if directory_offset.saturating_add(directory_size) > length {
        return Err(invalid("the zip directory lies outside the archive"));
    }
    file.seek(SeekFrom::Start(directory_offset))?;
    let mut directory = vec![0; usize::try_from(directory_size).unwrap_or(0)];
    file.read_exact(&mut directory)?;
    let mut at = 0usize;
    for _ in 0..count {
        if directory.len() < at + 46 || directory[at..at + 4] != [0x50, 0x4b, 0x01, 0x02] {
            return Err(invalid("a zip directory entry is malformed"));
        }
        let method = le16(&directory, at + 10);
        let compressed = le32(&directory, at + 20);
        let name_length = usize::try_from(le16(&directory, at + 28)).unwrap_or(0);
        let extra_length = usize::try_from(le16(&directory, at + 30)).unwrap_or(0);
        let comment_length = usize::try_from(le16(&directory, at + 32)).unwrap_or(0);
        let local = le32(&directory, at + 42);
        let name = directory
            .get(at + 46..at + 46 + name_length)
            .map(|name| String::from_utf8_lossy(name).replace('\\', "/"))
            .ok_or_else(|| invalid("a zip entry name is cut off"))?;
        at += 46 + name_length + extra_length + comment_length;
        if member_path(&name, 0).as_deref() != Some(member) {
            continue;
        }
        let mut header = [0u8; 30];
        file.seek(SeekFrom::Start(local))?;
        file.read_exact(&mut header)?;
        if header[..4] != [0x50, 0x4b, 0x03, 0x04] {
            return Err(invalid("a zip local header is malformed"));
        }
        let data = local + 30 + le16(&header, 26) + le16(&header, 28);
        file.seek(SeekFrom::Start(data))?;
        let raw = (&mut file).take(compressed);
        let mut output = fs::File::create(out)?;
        match method {
            0 => {
                io::copy(&mut io::BufReader::new(raw), &mut output)?;
            }
            8 => {
                let mut inflate = flate2::read::DeflateDecoder::new(io::BufReader::new(raw));
                io::copy(&mut inflate, &mut output)?;
            }
            other => {
                return Err(invalid(&format!(
                    "{member} uses zip compression method {other}, which is not read"
                )))
            }
        }
        return output.flush();
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("the archive has no {member}"),
    ))
}

#[cfg(unix)]
pub(super) fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // Owner read and write always; execute when the archive says so; nothing set-id.
    let mode = (mode & 0o755) | 0o600;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
pub(super) fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// Builds archives for tests: a gzipped ustar tar and a stored or deflated zip.
#[cfg(test)]
pub(crate) mod fixtures {
    use std::io::Write;

    fn tar_header(name: &str, size: usize, kind: u8, mode: u32) -> [u8; 512] {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..107].copy_from_slice(format!("{mode:07o}").as_bytes());
        header[108..115].copy_from_slice(b"0000000");
        header[116..123].copy_from_slice(b"0000000");
        header[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
        header[136..147].copy_from_slice(b"00000000000");
        header[156] = kind;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        header[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        header
    }

    /// A gzipped tar of `files` (name, contents, tar type: `b'0'` file, `b'2'` link).
    pub(crate) fn tar_gz(files: &[(&str, &[u8], u8)]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, contents, kind) in files {
            tar.extend_from_slice(&tar_header(name, contents.len(), *kind, 0o755));
            tar.extend_from_slice(contents);
            tar.resize(tar.len().div_ceil(512) * 512, 0);
        }
        tar.extend_from_slice(&[0u8; 1024]);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar).unwrap();
        encoder.finish().unwrap()
    }

    /// A zip of `files`, each deflated.
    pub(crate) fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut directory = Vec::new();
        for (name, contents) in files {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(contents).unwrap();
            let compressed = encoder.finish().unwrap();
            let offset = u32::try_from(out.len()).unwrap();
            let sizes = |buffer: &mut Vec<u8>| {
                buffer.extend_from_slice(&0u32.to_le_bytes()); // crc (not checked)
                buffer.extend_from_slice(&u32::try_from(compressed.len()).unwrap().to_le_bytes());
                buffer.extend_from_slice(&u32::try_from(contents.len()).unwrap().to_le_bytes());
            };
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
            sizes(&mut out);
            out.extend_from_slice(&u16::try_from(name.len()).unwrap().to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&compressed);
            directory
                .extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
            sizes(&mut directory);
            directory.extend_from_slice(&u16::try_from(name.len()).unwrap().to_le_bytes());
            directory.extend_from_slice(&[0; 12]);
            directory.extend_from_slice(&offset.to_le_bytes());
            directory.extend_from_slice(name.as_bytes());
        }
        let directory_offset = u32::try_from(out.len()).unwrap();
        let count = u16::try_from(files.len()).unwrap();
        out.extend_from_slice(&directory);
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&u32::try_from(directory.len()).unwrap().to_le_bytes());
        out.extend_from_slice(&directory_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::fixtures::{tar_gz, zip};
    use super::{extract_tar_gz_member, extract_zip_member, member_path, unpack_tar_gz};

    fn temp(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("neuron-mcp-archive-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn member_paths_drop_the_package_folder_and_never_leave_the_destination() {
        assert_eq!(
            member_path("package/dist/index.js", 1).as_deref(),
            Some("dist/index.js")
        );
        assert_eq!(member_path("./package/a", 1).as_deref(), Some("a"));
        assert_eq!(member_path("package/", 1), None);
        for bad in ["package/../../x", "package/a/../../b", "package/C:/x"] {
            assert_eq!(member_path(bad, 1), None, "{bad}");
        }
    }

    #[test]
    fn a_tarball_unpacks_files_only_inside_the_folder() {
        let dir = temp("tar");
        let archive = dir.join("pkg.tgz");
        fs::write(
            &archive,
            tar_gz(&[
                ("package/package.json", b"{}", b'0'),
                ("package/dist/index.js", b"console.log(1)", b'0'),
                ("package/../escape.txt", b"no", b'0'),
                ("package/link", b"", b'2'),
            ]),
        )
        .unwrap();
        let out = dir.join("out");
        let written = unpack_tar_gz(&archive, &out, 1).unwrap();
        assert_eq!(written, vec!["package.json", "dist/index.js"]);
        assert_eq!(
            fs::read_to_string(out.join("dist/index.js")).unwrap(),
            "console.log(1)"
        );
        assert!(!dir.join("escape.txt").exists());
        assert!(!out.join("link").exists());

        let single = dir.join("single");
        extract_tar_gz_member(&archive, "package/dist/index.js", &single).unwrap();
        assert_eq!(fs::read(&single).unwrap(), b"console.log(1)");
        assert!(extract_tar_gz_member(&archive, "missing", &single).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_zip_member_is_inflated_and_a_missing_one_is_an_error() {
        let dir = temp("zip");
        let archive = dir.join("asset.zip");
        let program = b"MZ program bytes".repeat(100);
        fs::write(
            &archive,
            zip(&[("LICENSE", b"MIT"), ("server.exe", &program)]),
        )
        .unwrap();
        let out = dir.join("server.exe");
        extract_zip_member(&archive, "server.exe", &out).unwrap();
        assert_eq!(fs::read(&out).unwrap(), program);
        assert!(extract_zip_member(&archive, "other.exe", &out).is_err());
        fs::write(&archive, b"not a zip").unwrap();
        assert!(extract_zip_member(&archive, "server.exe", &out).is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
