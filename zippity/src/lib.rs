use assert2::assert;
use core::panic;
use lru::LruCache;
use packed_struct::{PackedStruct, PackedStructSlice};
use pin_project::pin_project;
use std::collections::BTreeMap;
use std::future::Future;
use std::io::{Error, Result, SeekFrom};
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use structs::PackedStructZippityExt;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};

mod structs;

/// Minimum version needed to extract the zip64 extensions required by zippity
pub const ZIP64_VERSION_TO_EXTRACT: u16 = 45;

pub trait EntryData {
    type Reader: AsyncRead;
    type ReaderFuture: Future<Output = Result<Self::Reader>>;

    fn get_size(&self) -> u64;
    fn get_reader(&self) -> Self::ReaderFuture;
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
struct CrcCacheKey {}

pub struct CrcCache(LruCache<CrcCacheKey, u32>);

impl CrcCache {
    pub fn new(limit: NonZeroUsize) -> Self {
        CrcCache(LruCache::new(limit))
    }

    pub fn unbounded() -> Self {
        CrcCache(LruCache::unbounded())
    }
}

impl EntryData for () {
    type Reader = std::io::Cursor<&'static [u8]>;
    type ReaderFuture = std::future::Ready<Result<Self::Reader>>;

    fn get_size(&self) -> u64 {
        0
    }

    fn get_reader(&self) -> Self::ReaderFuture {
        std::future::ready(Ok(std::io::Cursor::new(&[])))
    }
}

impl<'a> EntryData for &'a [u8] {
    type Reader = std::io::Cursor<&'a [u8]>;
    type ReaderFuture = std::future::Ready<Result<Self::Reader>>;

    fn get_size(&self) -> u64 {
        self.len() as u64
    }

    fn get_reader(&self) -> Self::ReaderFuture {
        std::future::ready(Ok(std::io::Cursor::new(self)))
    }
}

#[derive(Clone, Debug)]
struct BuilderEntry<D> {
    data: D,
}

impl<D: EntryData> BuilderEntry<D> {
    fn get_local_size(&self, name: &str) -> u64 {
        let local_header = structs::LocalFileHeader::packed_size();
        let filename = name.len() as u64;
        let data = self.data.get_size();
        let data_descriptor = structs::DataDescriptor64::packed_size();

        let size = local_header + filename + data + data_descriptor;
        size
    }

    fn get_cd_header_size(&self, name: &str) -> u64 {
        let filename = name.len() as u64;
        let cd_entry = structs::CentralDirectoryHeader::packed_size();

        let size = cd_entry + filename;
        size
    }
}

#[derive(Clone, Debug)]
struct ReaderEntry<D> {
    name: String,
    data: D,
    size: u64,
    offset: u64,
    crc32: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Builder<D: EntryData> {
    entries: BTreeMap<String, BuilderEntry<D>>,
}

impl<D: EntryData> Builder<D> {
    pub fn new() -> Self {
        Builder {
            entries: BTreeMap::new(),
        }
    }

    pub fn add_entry<T: Into<D>>(&mut self, name: String, data: T) {
        let data = data.into();
        self.entries
            .insert(name, BuilderEntry { data: data.into() });
    }

    pub fn build(self) -> Reader<D> {
        // TODO: Allow filling CRCs from cache.
        let mut offset: u64 = 0;
        let mut cd_size: u64 = 0;
        let entries: Vec<_> = self
            .entries
            .into_iter()
            .map(|(name, entry)| {
                let size = entry.get_local_size(&name);
                let offset_copy = offset;
                offset += size;
                cd_size += entry.get_cd_header_size(&name);
                ReaderEntry {
                    name,
                    data: entry.data,
                    size,
                    offset: offset_copy,
                    crc32: None,
                }
            })
            .collect();

        let cd_offset = offset;
        let eocd_size = structs::EndOfCentralDirectory::packed_size();
        let total_size = cd_offset + cd_size + eocd_size;
        let current_chunk = Chunk::new(&entries);

        Reader {
            cd_offset,
            cd_size,
            total_size,

            entries,

            read_state: ReadState {
                current_chunk,
                pack_buffer: Vec::new(),
                to_skip: 0,
            },
            pinned: ReaderPinned::Nothing,
        }
    }
}

#[derive(Debug)]
enum Chunk {
    LocalHeader {
        entry_index: usize,
    },
    FileData {
        entry_index: usize,
        hasher: crc32fast::Hasher,
        size: u64,
    },
    DataDescriptor {
        entry_index: usize,
    },
    CDFileHeader {
        entry_index: usize,
    },
    EOCD,
    Finished,
}

impl Chunk {
    fn new<D>(entries: &Vec<ReaderEntry<D>>) -> Chunk {
        if entries.is_empty() {
            Chunk::EOCD
        } else {
            Chunk::LocalHeader { entry_index: 0 }
        }
    }

    fn size<D: EntryData>(&self, entries: &Vec<ReaderEntry<D>>) -> u64 {
        match self {
            Chunk::LocalHeader { entry_index } => {
                structs::LocalFileHeader::packed_size()
                    + entries[*entry_index].name.len() as u64
                    + structs::Zip64ExtraField::packed_size()
            }
            Chunk::FileData {
                entry_index,
                hasher: _,
                size: _,
            } => entries[*entry_index].data.get_size(),
            Chunk::DataDescriptor { entry_index: _ } => structs::DataDescriptor64::packed_size(),
            Chunk::CDFileHeader { entry_index } => {
                structs::CentralDirectoryHeader::packed_size()
                    + entries[*entry_index].name.len() as u64
                    + structs::Zip64ExtraField::packed_size()
            }
            Chunk::EOCD => structs::EndOfCentralDirectory::packed_size(),
            Chunk::Finished => 0,
        }
    }

    fn next<D>(&self, entries: &Vec<ReaderEntry<D>>) -> Chunk {
        match self {
            Chunk::LocalHeader { entry_index } => Chunk::FileData {
                entry_index: *entry_index,
                hasher: crc32fast::Hasher::new(),
                size: 0,
            },
            Chunk::FileData {
                entry_index,
                hasher: _,
                size: _,
            } => Chunk::DataDescriptor {
                entry_index: *entry_index,
            },
            Chunk::DataDescriptor { entry_index } => {
                let entry_index = *entry_index + 1;
                if entry_index < entries.len() {
                    Chunk::LocalHeader { entry_index }
                } else {
                    Chunk::CDFileHeader { entry_index: 0 }
                }
            }
            Chunk::CDFileHeader { entry_index } => {
                let entry_index = *entry_index + 1;
                if entry_index < entries.len() {
                    Chunk::CDFileHeader { entry_index }
                } else {
                    Chunk::EOCD
                }
            }
            Chunk::EOCD => Chunk::Finished,
            Chunk::Finished => Chunk::Finished,
        }
    }
}

#[pin_project(project = ReaderPinnedProj)]
enum ReaderPinned<D: EntryData> {
    Nothing,
    ReaderFuture(#[pin] D::ReaderFuture),
    FileReader(#[pin] D::Reader),
}

/// Parts of the state of reader that don't need pinning.
/// As a result, these can be accessed using a mutable reference
/// and can have mutable methods
struct ReadState {
    /// Which chunk we are currently reading
    current_chunk: Chunk,
    /// Buffer for packing structures that don't fit into the output as a whole.
    pack_buffer: Vec<u8>,
    /// How many bytes must be skipped, counted from the start of the current chunk
    to_skip: u64,
}

#[pin_project]
pub struct Reader<D: EntryData> {
    // These should be immutable dring the Reader lifetime
    cd_offset: u64,
    cd_size: u64,
    total_size: u64,

    /// Vector of entries and their offsets (counted from start of file)
    entries: Vec<ReaderEntry<D>>,

    /// Parts of mutable state that don't need pinning
    read_state: ReadState,

    /// Nested futures that need to be kept pinned, also used as a secondary state,
    #[pin]
    pinned: ReaderPinned<D>,
}

macro_rules! read_ready {
    ($x:expr) => {
        if !$x {
            return false;
        }
    };
}

impl ReadState {
    /// Write as much of ps as possible into output, spill the rest to the overflow buffer.
    /// Overflow buffer must be empty.
    fn read_packed_struct<F, P>(&mut self, ps_generator: F, output: &mut ReadBuf<'_>) -> bool
    where
        F: FnOnce() -> P,
        P: PackedStruct,
    {
        let size = P::packed_size() as usize;

        if self.to_skip > size as u64 {
            self.to_skip -= size as u64;
            true
        } else {
            let ps = ps_generator();

            if (self.to_skip == 0u64) & (output.remaining() >= size) {
                // Shortcut: Serialize directly to output
                let output_slice = output.initialize_unfilled_to(size);
                ps.pack_to_slice(output_slice).unwrap();
                output.advance(size);
                true
            } else {
                // The general way: Pack to the buffer and write bytes.
                self.pack_buffer.resize(size, 0);
                ps.pack_to_slice(&mut self.pack_buffer.as_mut_slice())
                    .unwrap();

                let skip = size.min(self.to_skip as usize);
                let write = output.remaining().min(size - skip);
                self.to_skip -= skip as u64;
                output.put_slice(self.pack_buffer.get(skip..(skip + write)).unwrap());

                let is_done = (skip + write) == size;
                is_done
            }
        }
    }

    /// Read as much of a string slice as possible into output.
    /// Does not use the overflow buffer.
    /// Returns true if the whole slice was successfully written, false if we ran out of space in the output.
    fn read_str(&mut self, s: &str, output: &mut ReadBuf<'_>) -> bool {
        let bytes = s.as_bytes();

        if self.to_skip > bytes.len() as u64 {
            self.to_skip -= bytes.len() as u64;
            true
        } else {
            let skip = bytes.len().min(self.to_skip as usize);
            let write = output.remaining().min(bytes.len() - skip);
            self.to_skip -= skip as u64;
            output.put_slice(bytes.get(skip..(skip + write)).unwrap());

            let is_done = (skip + write) == bytes.len();
            is_done
        }
    }

    fn read_local_header<D>(&mut self, entry: &ReaderEntry<D>, output: &mut ReadBuf<'_>) -> bool {
        read_ready!(self.read_packed_struct(
            || structs::LocalFileHeader {
                signature: structs::LocalFileHeader::SIGNATURE,
                version_to_extract: ZIP64_VERSION_TO_EXTRACT,
                flags: structs::GpBitFlag {
                    use_data_descriptor: true,
                },
                compression: structs::Compression::Store,
                last_mod_time: 0,
                last_mod_date: 0,
                crc32: 0,
                compressed_size: 0xffffffff,
                uncompressed_size: 0xffffffff,
                file_name_len: entry.name.len() as u16,
                extra_field_len: structs::Zip64ExtraField::packed_size() as u16,
            },
            output
        ));
        read_ready!(self.read_str(&entry.name, output));
        self.read_packed_struct(
            || structs::Zip64ExtraField {
                tag: structs::Zip64ExtraField::TAG,
                size: structs::Zip64ExtraField::packed_size() as u16 - 4,
                uncompressed_size: 0,
                compressed_size: 0,
                offset: entry.offset,
                disk_start_number: 0,
            },
            output,
        )
    }

    fn read_file_data<D: EntryData>(
        &mut self,
        entry: &mut ReaderEntry<D>,
        hasher: &mut crc32fast::Hasher,
        processed_size: &mut u64,
        mut pinned: Pin<&mut ReaderPinned<D>>,
        ctx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<Result<bool>> {
        let expected_size = entry.data.get_size();

        assert!(self.to_skip < expected_size);

        if let ReaderPinnedProj::Nothing = pinned.as_mut().project() {
            let reader_future = entry.data.get_reader();
            pinned.set(ReaderPinned::ReaderFuture(reader_future));
        }

        if let ReaderPinnedProj::ReaderFuture(ref mut reader_future) = pinned.as_mut().project() {
            let reader = ready!(reader_future.as_mut().poll(ctx))?;
            pinned.set(ReaderPinned::FileReader(reader));
        }

        let ReaderPinnedProj::FileReader(ref mut file_reader) = pinned.as_mut().project() else {
            panic!("FileReader must be available at this point because of the preceding two conditions");
        };

        // TODO: We might want to decide to not recompute the CRC and seek instead
        while self.to_skip > 0 {
            // Construct a temporary output buffer in the unused part of the real output buffer,
            // but not large enough to read more than the ammount to skip
            let mut tmp_output = output.take(self.to_skip.try_into().unwrap_or(usize::MAX));
            assert!(tmp_output.filled().is_empty());

            ready!(file_reader.as_mut().poll_read(ctx, &mut tmp_output))?;

            hasher.update(tmp_output.filled());
            *processed_size += tmp_output.filled().len() as u64;
            self.to_skip -= tmp_output.filled().len() as u64;
        }

        let remaining_before_poll = output.remaining();
        ready!(file_reader.as_mut().poll_read(ctx, output))?;

        if output.remaining() == remaining_before_poll {
            // Nothing was output => we read everything in the file already

            pinned.set(ReaderPinned::Nothing);

            // Cloning as a workaround -- finalize consumes, but we only borrowed the hasher mutably
            entry.crc32 = Some(hasher.clone().finalize());

            if *processed_size == expected_size {
                Poll::Ready(Ok(true)) // We're done with this state
            } else {
                Poll::Ready(Err(Error::other(Box::new(ZippityError::LengthMismatch {
                    entry_name: entry.name.clone(),
                    expected_size,
                    actual_size: *processed_size,
                }))))
            }
        } else {
            let written_chunk_size = remaining_before_poll - output.remaining();
            let buf_slice = output.filled();
            let written_chunk = &buf_slice[(buf_slice.len() - written_chunk_size)..];
            assert!(written_chunk_size == written_chunk.len());

            hasher.update(written_chunk);
            *processed_size += written_chunk_size as u64;

            Poll::Ready(Ok(false))
        }
    }

    fn read_data_descriptor<D: EntryData>(
        &mut self,
        entry: &ReaderEntry<D>,
        output: &mut ReadBuf<'_>,
    ) -> bool {
        self.read_packed_struct(
            || structs::DataDescriptor64 {
                signature: structs::DataDescriptor64::SIGNATURE,
                crc32: entry.crc32.unwrap(),
                compressed_size: entry.data.get_size(),
                uncompressed_size: entry.data.get_size(),
            },
            output,
        )
    }

    fn read_cd_file_header<D>(&mut self, entry: &ReaderEntry<D>, output: &mut ReadBuf<'_>) -> bool {
        read_ready!(self.read_packed_struct(
            || structs::CentralDirectoryHeader {
                signature: structs::CentralDirectoryHeader::SIGNATURE,
                version_made_by: structs::VersionMadeBy {
                    os: structs::VersionMadeByOs::UNIX,
                    spec_version: ZIP64_VERSION_TO_EXTRACT as u8,
                },
                version_to_extract: ZIP64_VERSION_TO_EXTRACT,
                flags: 0,
                compression: structs::Compression::Store,
                last_mod_time: 0,
                last_mod_date: 0,
                crc32: entry.crc32.unwrap(),
                compressed_size: 0xffffffff,
                uncompressed_size: 0xffffffff,
                file_name_len: entry.name.len() as u16,
                extra_field_len: structs::Zip64ExtraField::packed_size() as u16,
                file_comment_length: 0,
                disk_number_start: 0xffff,
                internal_attributes: 0,
                external_attributes: 0,
                local_header_offset: 0xffffffff,
            },
            output,
        ));
        read_ready!(self.read_str(&entry.name, output));
        self.read_packed_struct(
            || structs::Zip64ExtraField {
                tag: structs::Zip64ExtraField::TAG,
                size: structs::Zip64ExtraField::packed_size() as u16 - 4,
                uncompressed_size: 0,
                compressed_size: 0,
                offset: entry.offset,
                disk_start_number: 0,
            },
            output,
        )
    }

    fn read_eocd(&mut self, output: &mut ReadBuf<'_>) -> bool {
        self.read_packed_struct(
            || structs::EndOfCentralDirectory {
                signature: structs::EndOfCentralDirectory::SIGNATURE,
                this_disk_number: 0xffff,
                start_of_cd_disk_number: 0xffff,
                this_cd_entry_count: 0xffff,
                total_cd_entry_count: 0xffff,
                size_of_cd: 0xffffffff,
                cd_offset: 0xffffffff,
                file_comment_length: 0,
            },
            output,
        )
    }

    fn read<D: EntryData>(
        &mut self,
        entries: &mut Vec<ReaderEntry<D>>,
        mut pinned: Pin<&mut ReaderPinned<D>>,
        ctx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let initial_remaining = buf.remaining();

        loop {
            if self.to_skip >= self.current_chunk.size(entries) {
                self.current_chunk = self.current_chunk.next(entries);
                continue;
            }

            let loop_remaining = buf.remaining();
            let state_done = match &mut self.current_chunk {
                Chunk::LocalHeader { entry_index } => {
                    let entry_index = *entry_index;
                    self.read_local_header(&entries[entry_index], buf)
                }
                Chunk::FileData {
                    entry_index,
                    hasher,
                    size,
                } => {
                    let entry_index = *entry_index;
                    if buf.remaining() != initial_remaining {
                        // We have already written something into the buffer -> interrupt this call, because
                        // we might need to return Pending when reading the file data
                        return Poll::Ready(Ok(()));
                    }
                    let mut cloned_hasher = hasher.clone();
                    let read_result = self.read_file_data(
                        &mut entries[entry_index],
                        &mut cloned_hasher,
                        size,
                        pinned.as_mut(),
                        ctx,
                        buf,
                    );
                    *hasher = cloned_hasher;
                    ready!(read_result)?
                }
                Chunk::DataDescriptor { entry_index } => {
                    let entry_index = *entry_index;
                    self.read_data_descriptor(&entries[entry_index], buf)
                }
                Chunk::CDFileHeader { entry_index } => {
                    let entry_index = *entry_index;
                    self.read_cd_file_header(&entries[entry_index], buf)
                }
                Chunk::EOCD => self.read_eocd(buf),
                _ => return Poll::Ready(Ok(())),
            };

            let read_len = loop_remaining - buf.remaining();

            if state_done {
                self.to_skip = 0;
                self.current_chunk = self.current_chunk.next(entries);
            } else {
                self.to_skip += read_len as u64;
            }
        }
    }
}

impl<D: EntryData> Reader<D> {
    pub fn get_size(&self) -> u64 {
        self.total_size
    }
}

impl<D: EntryData> AsyncRead for Reader<D> {
    fn poll_read(
        self: Pin<&mut Self>,
        ctx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let projected = self.project();
        projected
            .read_state
            .read(projected.entries, projected.pinned, ctx, buf)
    }
}

impl<D: EntryData> AsyncSeek for Reader<D> {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
        todo!()
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        todo!()
    }
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum ZippityError {
    #[error("Entry {entry_name} reports length {expected_size} B, but was {actual_size} B")]
    LengthMismatch {
        entry_name: String,
        expected_size: u64,
        actual_size: u64,
    },
}

#[cfg(test)]
mod test {
    use super::*;
    use assert2::assert;
    use proptest::strategy::{Just, Strategy};
    use std::{collections::HashMap, fmt::format, future::Future, io::ErrorKind};
    use test_strategy::proptest;
    use tokio::io::AsyncReadExt;
    use zip::read::ZipArchive;

    async fn read_to_vec(reader: impl AsyncRead, read_size: usize) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();
        let mut reader = Box::pin(reader);

        loop {
            let size_before = buffer.len();
            buffer.resize(size_before + read_size, 0);
            let (_, write_slice) = buffer.split_at_mut(size_before);

            let size_read = reader.read(write_slice).await?;

            buffer.truncate(size_before + size_read);

            if size_read == 0 {
                return Ok(buffer);
            }
        }
    }

    fn unasync<Fut: Future>(fut: Fut) -> Fut::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    fn content_strategy() -> impl Strategy<Value = HashMap<String, Vec<u8>>> {
        proptest::collection::hash_map(
            // We're limiting the character set significantly, because the zip crate we use for verification
            // does not handle unicode filenames well.
            r"[a-zA-Z0-91235678!@#$%^&U*/><\\\[\]]{1,500}",
            proptest::collection::vec(0u8..255u8, 0..1024),
            0..100,
        )
    }

    #[proptest]
    fn test_empty_archive(#[strategy(1usize..8192usize)] read_size: usize) {
        let zippity: Reader<()> = Builder::new().build();
        let size = zippity.get_size();

        let buf = unasync(read_to_vec(zippity, read_size)).unwrap();

        assert!(size == (buf.len() as u64));

        let unpacked = ZipArchive::new(std::io::Cursor::new(buf)).expect("Should be a valid zip");
        assert!(unpacked.is_empty());
    }

    #[proptest]
    fn test_unzip_with_data(
        #[strategy(content_strategy())] content: HashMap<String, Vec<u8>>,
        #[strategy(1usize..8192usize)] read_size: usize,
    ) {
        let mut builder: Builder<&[u8]> = Builder::new();

        content.iter().for_each(|(name, value)| {
            builder.add_entry(name.clone(), value.as_ref());
        });

        let zippity = builder.build();
        let size = zippity.get_size();

        let buf = unasync(read_to_vec(zippity, read_size)).unwrap();

        assert!(size == (buf.len() as u64));

        let mut unpacked =
            ZipArchive::new(std::io::Cursor::new(buf)).expect("Should be a valid zip");
        assert!(unpacked.len() == content.len());

        let mut unpacked_content = HashMap::new();
        for i in 0..unpacked.len() {
            let mut zipfile = unpacked.by_index(i).unwrap();
            let name = std::str::from_utf8(zipfile.name_raw()).unwrap().to_string();
            let mut file_content = Vec::new();
            use std::io::Read;
            zipfile.read_to_end(&mut file_content).unwrap();

            unpacked_content.insert(name, file_content);
        }
    }

    #[test]
    fn bad_size() {
        /// Struct that reports data size 100, but actually its 1
        struct BadSize();
        impl EntryData for BadSize {
            type Reader = std::io::Cursor<&'static [u8]>;
            type ReaderFuture = std::future::Ready<Result<Self::Reader>>;

            fn get_size(&self) -> u64 {
                100
            }

            fn get_reader(&self) -> Self::ReaderFuture {
                std::future::ready(Ok(std::io::Cursor::new(&[5])))
            }
        }

        let mut builder: Builder<BadSize> = Builder::new();
        builder.add_entry("xxx".into(), BadSize());

        let zippity = builder.build();
        let e = unasync(read_to_vec(zippity, 1024)).unwrap_err();

        assert!(e.kind() == ErrorKind::Other);
        let message = format!("{}", e.into_inner().unwrap());

        assert!(message.contains("xxx"));
    }
}
