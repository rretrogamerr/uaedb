use std::fs::File;
use std::io::{self, BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

pub const COMP_MASK: u32 = 0x3F;
pub const COMP_NONE: u32 = 0;
pub const COMP_LZMA: u32 = 1;
pub const COMP_LZ4: u32 = 2;
pub const COMP_LZ4HC: u32 = 3;
pub const COMP_LZHAM: u32 = 4;

const FLAG_BLOCKS_AND_DIR: u32 = 0x40;
const FLAG_BLOCKS_INFO_AT_END: u32 = 0x80;
const FLAG_BLOCK_INFO_NEED_PADDING: u32 = 0x200;
const FLAG_ENCRYPTION_OLD: u32 = 0x200;
const FLAG_ENCRYPTION_NEW: u32 = 0x1400;

#[derive(Debug, Clone)]
pub struct BlockInfo {
    pub uncompressed_size: u32,
    pub compressed_size: u32,
    pub flags: u16,
}

#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    pub offset: u64,
    pub size: u64,
    pub flags: u32,
    pub path: String,
}

#[derive(Debug)]
pub struct UnityFsBundle {
    signature: String,
    version: u32,
    version_player: String,
    version_engine: String,
    flags: u32,
    uses_block_alignment: bool,
    data_start: u64,
    block_info_flags: u16,
    blocks: Vec<BlockInfo>,
    entries: Vec<DirectoryEntry>,
}

impl UnityFsBundle {
    pub fn read(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("Open bundle: {}", path.display()))?;
        let file_len = file
            .metadata()
            .with_context(|| format!("Stat bundle: {}", path.display()))?
            .len();
        let mut reader = BufReader::new(file);

        let signature = read_string_to_null(&mut reader)?;
        if signature != "UnityFS" {
            bail!("Unsupported bundle signature: {}", signature);
        }

        let version = read_u32_be(&mut reader)?;
        let version_player = read_string_to_null(&mut reader)?;
        let version_engine = read_string_to_null(&mut reader)?;
        let _size = read_u64_be(&mut reader)?;
        let compressed_block_info_size = read_u32_be(&mut reader)?;
        let uncompressed_block_info_size = read_u32_be(&mut reader)?;
        let flags = read_u32_be(&mut reader)?;

        let engine_version = parse_engine_version(&version_engine);
        let uses_new_flags = engine_version
            .map(uses_new_archive_flags)
            .unwrap_or(false);

        let encryption_flag = if uses_new_flags {
            FLAG_ENCRYPTION_NEW
        } else {
            FLAG_ENCRYPTION_OLD
        };
        if flags & encryption_flag != 0 {
            bail!("Encrypted asset bundles are not supported.");
        }

        let mut uses_block_alignment = false;
        if version >= 7 {
            align_reader(&mut reader, 16)?;
            uses_block_alignment = true;
        } else if engine_version.map_or(false, |v| v >= (2019, 4, 0)) {
            let pre_align = reader.stream_position()?;
            let padding = padding_for_alignment(pre_align, 16);
            if padding > 0 {
                let mut buf = vec![0u8; padding as usize];
                reader.read_exact(&mut buf)?;
                if buf.iter().all(|b| *b == 0) {
                    uses_block_alignment = true;
                } else {
                    reader.seek(SeekFrom::Start(pre_align))?;
                }
            }
        }

        let start = reader.stream_position()?;
        let block_info_at_end = flags & FLAG_BLOCKS_INFO_AT_END != 0;
        let mut block_info_bytes = vec![0u8; compressed_block_info_size as usize];

        if block_info_at_end {
            let block_info_offset = file_len
                .checked_sub(compressed_block_info_size as u64)
                .context("Compute block info offset")?;
            let mut file = reader.into_inner();
            file.seek(SeekFrom::Start(block_info_offset))?;
            file.read_exact(&mut block_info_bytes)?;
            file.seek(SeekFrom::Start(start))?;
            reader = BufReader::new(file);
        } else {
            reader.read_exact(&mut block_info_bytes)?;
        }

        let block_info_bytes = decompress_block_info(
            &block_info_bytes,
            uncompressed_block_info_size as usize,
            flags,
        )?;

        let mut info_reader = Cursor::new(block_info_bytes);
        let mut hash = [0u8; 16];
        info_reader.read_exact(&mut hash)?;
        let block_count = read_i32_be(&mut info_reader)? as usize;
        let mut blocks = Vec::with_capacity(block_count);
        for _ in 0..block_count {
            blocks.push(BlockInfo {
                uncompressed_size: read_u32_be(&mut info_reader)?,
                compressed_size: read_u32_be(&mut info_reader)?,
                flags: read_u16_be(&mut info_reader)?,
            });
        }

        let entry_count = read_i32_be(&mut info_reader)? as usize;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            entries.push(DirectoryEntry {
                offset: read_i64_be(&mut info_reader)? as u64,
                size: read_i64_be(&mut info_reader)? as u64,
                flags: read_u32_be(&mut info_reader)?,
                path: read_string_to_null(&mut info_reader)?,
            });
        }

        let block_info_flags = blocks.first().map(|b| b.flags).unwrap_or(0);

        if uses_new_flags && flags & FLAG_BLOCK_INFO_NEED_PADDING != 0 {
            align_reader(&mut reader, 16)?;
        }

        let data_start = reader.stream_position()?;

        Ok(Self {
            signature,
            version,
            version_player,
            version_engine,
            flags,
            uses_block_alignment,
            data_start,
            block_info_flags,
            blocks,
            entries,
        })
    }

    pub fn entries(&self) -> &[DirectoryEntry] {
        &self.entries
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }

    pub fn block_info_flags(&self) -> u16 {
        self.block_info_flags
    }

    pub fn decompress_to_file(&self, input_path: &Path, output_path: &Path) -> Result<()> {
        let mut input = BufReader::new(
            File::open(input_path).with_context(|| format!("Open bundle: {}", input_path.display()))?,
        );
        input.seek(SeekFrom::Start(self.data_start))?;

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Create dir: {}", parent.display()))?;
        }
        let mut output = BufWriter::new(
            File::create(output_path)
                .with_context(|| format!("Create output: {}", output_path.display()))?,
        );

        decompress_blocks_to_writer(&mut input, &mut output, &self.blocks)?;

        output.flush()?;
        Ok(())
    }

    pub fn unpack_to_file(&self, input_path: &Path, output_path: &Path) -> Result<()> {
        let mut input = BufReader::new(
            File::open(input_path).with_context(|| format!("Open bundle: {}", input_path.display()))?,
        );
        input.seek(SeekFrom::Start(self.data_start))?;

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Create dir: {}", parent.display()))?;
        }
        let mut output = BufWriter::new(
            File::create(output_path)
                .with_context(|| format!("Create output: {}", output_path.display()))?,
        );

        let data_flags = (self.flags & !COMP_MASK) | FLAG_BLOCKS_AND_DIR;
        let uncompressed_blocks: Vec<BlockInfo> = self
            .blocks
            .iter()
            .map(|block| BlockInfo {
                uncompressed_size: block.uncompressed_size,
                compressed_size: block.uncompressed_size,
                flags: clear_compression_flags(block.flags),
            })
            .collect();

        let block_info_bytes = build_block_info_bytes(&uncompressed_blocks, &self.entries)?;
        let block_info_size = block_info_bytes.len() as u32;

        write_string_to_null(&mut output, &self.signature)?;
        write_u32_be(&mut output, self.version)?;
        write_string_to_null(&mut output, &self.version_player)?;
        write_string_to_null(&mut output, &self.version_engine)?;

        let size_offset = output.stream_position()?;
        write_u64_be(&mut output, 0)?;
        write_u32_be(&mut output, block_info_size)?;
        write_u32_be(&mut output, block_info_size)?;
        write_u32_be(&mut output, data_flags)?;

        if self.uses_block_alignment {
            align_writer(&mut output, 16)?;
        }

        let block_info_at_end = data_flags & FLAG_BLOCKS_INFO_AT_END != 0;
        let block_info_need_padding = data_flags & FLAG_BLOCK_INFO_NEED_PADDING != 0;

        if block_info_at_end {
            if block_info_need_padding {
                align_writer(&mut output, 16)?;
            }
            decompress_blocks_to_writer(&mut input, &mut output, &self.blocks)?;
            output.write_all(&block_info_bytes)?;
        } else {
            output.write_all(&block_info_bytes)?;
            if block_info_need_padding {
                align_writer(&mut output, 16)?;
            }
            decompress_blocks_to_writer(&mut input, &mut output, &self.blocks)?;
        }

        output.flush()?;
        let mut file = output.into_inner()?;
        let end_pos = file.stream_position()?;
        file.seek(SeekFrom::Start(size_offset))?;
        write_u64_be(&mut file, end_pos)?;
        file.seek(SeekFrom::Start(end_pos))?;
        file.flush()?;
        Ok(())
    }

    pub fn extract_entry(
        &self,
        data_path: &Path,
        entry_index: usize,
        output_path: &Path,
    ) -> Result<()> {
        let entry = self
            .entries
            .get(entry_index)
            .context("Entry index out of range")?;
        let mut input = BufReader::new(
            File::open(data_path).with_context(|| format!("Open data: {}", data_path.display()))?,
        );
        input.seek(SeekFrom::Start(entry.offset))?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Create dir: {}", parent.display()))?;
        }
        let mut output = BufWriter::new(
            File::create(output_path)
                .with_context(|| format!("Create entry: {}", output_path.display()))?,
        );
        copy_exact(&mut input, &mut output, entry.size)?;
        output.flush()?;
        Ok(())
    }

    pub fn rebuild_data_file(
        &self,
        data_path: &Path,
        entry_index: usize,
        patched_entry: &Path,
        output_path: &Path,
    ) -> Result<Vec<DirectoryEntry>> {
        let mut input = BufReader::new(
            File::open(data_path).with_context(|| format!("Open data: {}", data_path.display()))?,
        );
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Create dir: {}", parent.display()))?;
        }
        let mut output = BufWriter::new(
            File::create(output_path)
                .with_context(|| format!("Create data: {}", output_path.display()))?,
        );

        let mut offset = 0u64;
        let mut new_entries = Vec::with_capacity(self.entries.len());
        let patched_size = std::fs::metadata(patched_entry)
            .with_context(|| format!("Stat patched entry: {}", patched_entry.display()))?
            .len();

        for (idx, entry) in self.entries.iter().enumerate() {
            let size = if idx == entry_index {
                let mut patched = BufReader::new(
                    File::open(patched_entry)
                        .with_context(|| format!("Open patched entry: {}", patched_entry.display()))?,
                );
                io::copy(&mut patched, &mut output)?;
                patched_size
            } else {
                input.seek(SeekFrom::Start(entry.offset))?;
                copy_exact(&mut input, &mut output, entry.size)?;
                entry.size
            };

            new_entries.push(DirectoryEntry {
                offset,
                size,
                flags: entry.flags,
                path: entry.path.clone(),
            });
            offset = offset
                .checked_add(size)
                .context("Data size overflow while rebuilding")?;
        }

        output.flush()?;
        Ok(new_entries)
    }

    pub fn write_bundle(
        &self,
        output_path: &Path,
        data_path: &Path,
        entries: &[DirectoryEntry],
        data_flags: u32,
        block_info_flags: u16,
    ) -> Result<()> {
        let compression = data_flags & COMP_MASK;
        if compression == COMP_LZHAM {
            bail!("LZHAM compression is not supported.");
        }
        if data_flags & FLAG_BLOCKS_AND_DIR == 0 {
            bail!("Bundle flags must include BlocksAndDirectoryInfoCombined (0x40).");
        }

        let block_info_at_end = data_flags & FLAG_BLOCKS_INFO_AT_END != 0;
        let block_info_need_padding = data_flags & FLAG_BLOCK_INFO_NEED_PADDING != 0;

        let work_dir = output_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let compressed_data_path = work_dir.join("uaedb-data.compressed");

        let block_info = compress_data_blocks(
            data_path,
            &compressed_data_path,
            block_info_flags,
        )?;

        let block_info_bytes = build_block_info_bytes(&block_info, entries)?;
        let uncompressed_block_info_size = block_info_bytes.len() as u32;
        let compressed_block_info_bytes =
            compress_block_info(&block_info_bytes, compression)?;
        let compressed_block_info_size = compressed_block_info_bytes.len() as u32;

        let mut output = BufWriter::new(
            File::create(output_path)
                .with_context(|| format!("Create bundle: {}", output_path.display()))?,
        );

        write_string_to_null(&mut output, &self.signature)?;
        write_u32_be(&mut output, self.version)?;
        write_string_to_null(&mut output, &self.version_player)?;
        write_string_to_null(&mut output, &self.version_engine)?;

        let size_offset = output.stream_position()?;
        write_u64_be(&mut output, 0)?;
        write_u32_be(&mut output, compressed_block_info_size)?;
        write_u32_be(&mut output, uncompressed_block_info_size)?;
        write_u32_be(&mut output, data_flags)?;

        if self.uses_block_alignment {
            align_writer(&mut output, 16)?;
        }

        if block_info_at_end {
            if block_info_need_padding {
                align_writer(&mut output, 16)?;
            }
            copy_file_to_writer(&compressed_data_path, &mut output)?;
            output.write_all(&compressed_block_info_bytes)?;
        } else {
            output.write_all(&compressed_block_info_bytes)?;
            if block_info_need_padding {
                align_writer(&mut output, 16)?;
            }
            copy_file_to_writer(&compressed_data_path, &mut output)?;
        }

        output.flush()?;
        let mut file = output.into_inner()?;
        let end_pos = file.stream_position()?;
        file.seek(SeekFrom::Start(size_offset))?;
        write_u64_be(&mut file, end_pos)?;
        file.seek(SeekFrom::Start(end_pos))?;
        file.flush()?;

        std::fs::remove_file(&compressed_data_path).ok();
        Ok(())
    }
}

fn build_block_info_bytes(blocks: &[BlockInfo], entries: &[DirectoryEntry]) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&[0u8; 16]);
    write_i32_be(&mut buffer, blocks.len() as i32)?;
    for block in blocks {
        write_u32_be(&mut buffer, block.uncompressed_size)?;
        write_u32_be(&mut buffer, block.compressed_size)?;
        write_u16_be(&mut buffer, block.flags)?;
    }
    write_i32_be(&mut buffer, entries.len() as i32)?;
    for entry in entries {
        write_i64_be(&mut buffer, entry.offset as i64)?;
        write_i64_be(&mut buffer, entry.size as i64)?;
        write_u32_be(&mut buffer, entry.flags)?;
        write_string_to_null(&mut buffer, &entry.path)?;
    }
    Ok(buffer)
}

fn compress_block_info(data: &[u8], compression: u32) -> Result<Vec<u8>> {
    match compression {
        COMP_NONE => Ok(data.to_vec()),
        COMP_LZ4 | COMP_LZ4HC => lz4_compress(data),
        COMP_LZMA => compress_lzma_bytes(data),
        COMP_LZHAM => bail!("LZHAM compression is not supported."),
        _ => bail!("Unknown compression flag: {}", compression),
    }
}

fn compress_data_blocks(
    data_path: &Path,
    output_path: &Path,
    block_info_flags: u16,
) -> Result<Vec<BlockInfo>> {
    let compression = (block_info_flags as u32) & COMP_MASK;
    let data_len = std::fs::metadata(data_path)
        .with_context(|| format!("Stat data: {}", data_path.display()))?
        .len();

    if compression == COMP_NONE || compression == COMP_LZMA {
        if data_len > u32::MAX as u64 {
            bail!("Data too large for single-block compression ({} bytes)", data_len);
        }
    }

    if compression == COMP_NONE {
        copy_file(data_path, output_path)?;
        return Ok(vec![BlockInfo {
            uncompressed_size: data_len as u32,
            compressed_size: data_len as u32,
            flags: block_info_flags,
        }]);
    }

    if compression == COMP_LZMA {
        compress_lzma_file(data_path, output_path)?;
        let compressed_len = std::fs::metadata(output_path)
            .with_context(|| format!("Stat compressed data: {}", output_path.display()))?
            .len();
        if compressed_len >= data_len {
            copy_file(data_path, output_path)?;
            return Ok(vec![BlockInfo {
                uncompressed_size: data_len as u32,
                compressed_size: data_len as u32,
                flags: clear_compression_flags(block_info_flags),
            }]);
        }
        return Ok(vec![BlockInfo {
            uncompressed_size: data_len as u32,
            compressed_size: compressed_len as u32,
            flags: block_info_flags,
        }]);
    }

    if compression != COMP_LZ4 && compression != COMP_LZ4HC {
        bail!("Unsupported compression flag: {}", compression);
    }

    let mut input = BufReader::new(
        File::open(data_path).with_context(|| format!("Open data: {}", data_path.display()))?,
    );
    let mut output = BufWriter::new(
        File::create(output_path)
            .with_context(|| format!("Create compressed data: {}", output_path.display()))?,
    );

    let chunk_size: usize = 0x0002_0000;
    let mut block_info = Vec::new();
    let mut remaining = data_len;

    while remaining > 0 {
        let size = std::cmp::min(remaining as usize, chunk_size);
        let mut buf = vec![0u8; size];
        input.read_exact(&mut buf)?;
        let compressed = lz4_compress(&buf)?;
        if compressed.len() > buf.len() {
            output.write_all(&buf)?;
            block_info.push(BlockInfo {
                uncompressed_size: buf.len() as u32,
                compressed_size: buf.len() as u32,
                flags: clear_compression_flags(block_info_flags),
            });
        } else {
            output.write_all(&compressed)?;
            block_info.push(BlockInfo {
                uncompressed_size: buf.len() as u32,
                compressed_size: compressed.len() as u32,
                flags: block_info_flags,
            });
        }
        remaining = remaining
            .checked_sub(size as u64)
            .context("Chunk size overflow")?;
    }

    output.flush()?;
    Ok(block_info)
}

fn decompress_block_info(data: &[u8], uncompressed_size: usize, flags: u32) -> Result<Vec<u8>> {
    let compression = flags & COMP_MASK;
    match compression {
        COMP_NONE => Ok(data.to_vec()),
        COMP_LZ4 | COMP_LZ4HC => lz4_decompress(data, uncompressed_size)
            .context("LZ4 decompress failed"),
        COMP_LZMA => lzma_decompress(data, uncompressed_size),
        COMP_LZHAM => bail!("LZHAM compression is not supported."),
        _ => bail!("Unknown compression flag: {}", compression),
    }
}

fn lz4_decompress(data: &[u8], size: usize) -> Result<Vec<u8>> {
    let size = i32::try_from(size).context("LZ4 size overflow")?;
    lz4::block::decompress(data, Some(size)).context("LZ4 decompress failed")
}

fn lz4_compress(data: &[u8]) -> Result<Vec<u8>> {
    // AssetsTools.NET Pack uses LZ4HC for bundle compression.
    lz4::block::compress(
        data,
        Some(lz4::block::CompressionMode::HIGHCOMPRESSION(9)),
        false,
    )
    .context("LZ4 compress failed")
}

fn lzma_decompress(data: &[u8], uncompressed_size: usize) -> Result<Vec<u8>> {
    if data.len() < 5 {
        bail!("LZMA data too small to contain header");
    }
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&data[..5]);
    header.extend_from_slice(&(uncompressed_size as u64).to_le_bytes());
    let mut reader = Cursor::new(header).chain(Cursor::new(&data[5..]));
    let stream = xz2::stream::Stream::new_lzma_decoder(u64::MAX)
        .context("Create LZMA decoder stream")?;
    let mut decoder = xz2::read::XzDecoder::new_stream(&mut reader, stream);
    let mut out = Vec::with_capacity(uncompressed_size);
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

fn lzma_decompress_to_writer<R: Read, W: Write>(
    header: &[u8; 5],
    compressed: &mut R,
    uncompressed_size: u64,
    out: &mut W,
) -> Result<()> {
    let mut header_buf = Vec::with_capacity(13);
    header_buf.extend_from_slice(header);
    header_buf.extend_from_slice(&uncompressed_size.to_le_bytes());
    let mut reader = Cursor::new(header_buf).chain(compressed);
    let stream = xz2::stream::Stream::new_lzma_decoder(u64::MAX)
        .context("Create LZMA decoder stream")?;
    let mut decoder = xz2::read::XzDecoder::new_stream(&mut reader, stream);
    io::copy(&mut decoder, out)?;
    Ok(())
}

fn compress_lzma_bytes(data: &[u8]) -> Result<Vec<u8>> {
    let options = lzma_options_unity().context("Create LZMA encoder options")?;
    let stream = xz2::stream::Stream::new_lzma_encoder(&options)
        .context("Create LZMA encoder stream")?;
    let mut encoder = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
    encoder.write_all(data)?;
    let encoded = encoder.finish()?;
    if encoded.len() < 13 {
        bail!("LZMA output too small");
    }
    let mut out = Vec::with_capacity(encoded.len().saturating_sub(8));
    out.extend_from_slice(&encoded[..5]);
    out.extend_from_slice(&encoded[13..]);
    Ok(out)
}

fn compress_lzma_file(input_path: &Path, output_path: &Path) -> Result<()> {
    let temp_path = output_path.with_extension("lzma.tmp");
    {
        let input = BufReader::new(
            File::open(input_path).with_context(|| format!("Open data: {}", input_path.display()))?,
        );
        let temp = BufWriter::new(
            File::create(&temp_path)
                .with_context(|| format!("Create temp: {}", temp_path.display()))?,
        );
        let options = lzma_options_unity().context("Create LZMA encoder options")?;
        let stream = xz2::stream::Stream::new_lzma_encoder(&options)
            .context("Create LZMA encoder stream")?;
        let mut encoder = xz2::write::XzEncoder::new_stream(temp, stream);
        io::copy(&mut input.take(u64::MAX), &mut encoder)?;
        let mut temp = encoder.finish()?;
        temp.flush()?;
    }

    let mut temp = BufReader::new(
        File::open(&temp_path).with_context(|| format!("Open temp: {}", temp_path.display()))?,
    );
    let mut output = BufWriter::new(
        File::create(output_path)
            .with_context(|| format!("Create output: {}", output_path.display()))?,
    );
    let mut header = [0u8; 13];
    temp.read_exact(&mut header)?;
    output.write_all(&header[..5])?;
    io::copy(&mut temp, &mut output)?;
    output.flush()?;
    std::fs::remove_file(&temp_path).ok();
    Ok(())
}

fn lzma_options_unity() -> Result<xz2::stream::LzmaOptions> {
    // Match Unity/AssetsTools.NET LZMA1 defaults (as used by UABEA).
    let mut options = xz2::stream::LzmaOptions::new_preset(6)?;
    options
        .dict_size(0x0080_0000)
        .literal_context_bits(3)
        .literal_position_bits(0)
        .position_bits(2)
        .mode(xz2::stream::Mode::Normal)
        .match_finder(xz2::stream::MatchFinder::BinaryTree4)
        .nice_len(123);
    Ok(options)
}

fn copy_file(input_path: &Path, output_path: &Path) -> Result<()> {
    let mut input = BufReader::new(
        File::open(input_path).with_context(|| format!("Open data: {}", input_path.display()))?,
    );
    let mut output = BufWriter::new(
        File::create(output_path)
            .with_context(|| format!("Create output: {}", output_path.display()))?,
    );
    io::copy(&mut input, &mut output)?;
    output.flush()?;
    Ok(())
}

fn copy_file_to_writer(input_path: &Path, output: &mut BufWriter<File>) -> Result<()> {
    let mut input = BufReader::new(
        File::open(input_path).with_context(|| format!("Open data: {}", input_path.display()))?,
    );
    io::copy(&mut input, output)?;
    Ok(())
}

fn copy_exact<R: Read, W: Write>(input: &mut R, output: &mut W, mut size: u64) -> Result<()> {
    let mut buffer = vec![0u8; 1024 * 1024];
    while size > 0 {
        let read_size = std::cmp::min(size as usize, buffer.len());
        input.read_exact(&mut buffer[..read_size])?;
        output.write_all(&buffer[..read_size])?;
        size -= read_size as u64;
    }
    Ok(())
}

fn decompress_blocks_to_writer<R: Read, W: Write>(
    input: &mut R,
    output: &mut W,
    blocks: &[BlockInfo],
) -> Result<()> {
    for block in blocks {
        let comp_flag = (block.flags as u32) & COMP_MASK;
        match comp_flag {
            COMP_NONE => {
                copy_exact(input, output, block.compressed_size as u64)?;
            }
            COMP_LZ4 | COMP_LZ4HC => {
                let mut compressed = vec![0u8; block.compressed_size as usize];
                input.read_exact(&mut compressed)?;
                let data = lz4_decompress(&compressed, block.uncompressed_size as usize)
                    .context("LZ4 decompress failed")?;
                output.write_all(&data)?;
            }
            COMP_LZMA => {
                if block.compressed_size < 5 {
                    bail!("LZMA block too small to contain header");
                }
                let mut header = [0u8; 5];
                input.read_exact(&mut header)?;
                let remaining = (block.compressed_size - 5) as u64;
                let mut limited = input.by_ref().take(remaining);
                lzma_decompress_to_writer(&header, &mut limited, block.uncompressed_size as u64, output)
                    .context("LZMA decompress failed")?;
            }
            COMP_LZHAM => bail!("LZHAM compression is not supported."),
            _ => bail!("Unknown compression flag: {}", comp_flag),
        }
    }
    Ok(())
}

fn read_string_to_null<R: Read>(reader: &mut R) -> Result<String> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 1];
    loop {
        reader.read_exact(&mut buf)?;
        if buf[0] == 0 {
            break;
        }
        bytes.push(buf[0]);
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn write_string_to_null<W: Write>(writer: &mut W, value: &str) -> Result<()> {
    writer.write_all(value.as_bytes())?;
    writer.write_all(&[0u8])?;
    Ok(())
}

fn read_u16_be<R: Read>(reader: &mut R) -> Result<u16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(u16::from_be_bytes(buf))
}

fn read_u32_be<R: Read>(reader: &mut R) -> Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_be_bytes(buf))
}

fn read_u64_be<R: Read>(reader: &mut R) -> Result<u64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_be_bytes(buf))
}

fn read_i32_be<R: Read>(reader: &mut R) -> Result<i32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(i32::from_be_bytes(buf))
}

fn read_i64_be<R: Read>(reader: &mut R) -> Result<i64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(i64::from_be_bytes(buf))
}

fn write_u16_be<W: Write>(writer: &mut W, value: u16) -> Result<()> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_u32_be<W: Write>(writer: &mut W, value: u32) -> Result<()> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_u64_be<W: Write>(writer: &mut W, value: u64) -> Result<()> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_i32_be<W: Write>(writer: &mut W, value: i32) -> Result<()> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_i64_be<W: Write>(writer: &mut W, value: i64) -> Result<()> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn align_reader<R: Read + Seek>(reader: &mut R, alignment: u64) -> Result<()> {
    let pos = reader.stream_position()?;
    let padding = padding_for_alignment(pos, alignment);
    if padding > 0 {
        reader.seek(SeekFrom::Current(padding as i64))?;
    }
    Ok(())
}

fn align_writer<W: Write + Seek>(writer: &mut W, alignment: u64) -> Result<()> {
    let pos = writer.stream_position()?;
    let padding = padding_for_alignment(pos, alignment);
    if padding > 0 {
        let zeros = vec![0u8; padding as usize];
        writer.write_all(&zeros)?;
    }
    Ok(())
}

fn padding_for_alignment(pos: u64, alignment: u64) -> u64 {
    let rem = pos % alignment;
    if rem == 0 {
        0
    } else {
        alignment - rem
    }
}

fn clear_compression_flags(flags: u16) -> u16 {
    flags & !(COMP_MASK as u16)
}

fn parse_engine_version(value: &str) -> Option<(u32, u32, u32)> {
    let mut nums = Vec::new();
    let mut current = String::new();
    for ch in value.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            if let Ok(num) = current.parse() {
                nums.push(num);
            }
            current.clear();
        }
        if nums.len() >= 3 {
            break;
        }
    }
    if !current.is_empty() && nums.len() < 3 {
        if let Ok(num) = current.parse() {
            nums.push(num);
        }
    }
    if nums.len() >= 3 {
        Some((nums[0], nums[1], nums[2]))
    } else {
        None
    }
}

fn uses_new_archive_flags(version: (u32, u32, u32)) -> bool {
    match version {
        (major, ..) if major < 2020 => false,
        (2020, minor, patch) if (minor, patch) < (3, 34) => false,
        (2021, minor, patch) if (minor, patch) < (3, 2) => false,
        (2022, minor, patch) if (minor, patch) < (1, 1) => false,
        _ => true,
    }
}
