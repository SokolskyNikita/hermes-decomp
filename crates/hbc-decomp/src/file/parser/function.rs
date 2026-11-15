use crate::error::Result;
use crate::format::{
    BytecodeHeader, FunctionHeader, FunctionHeaderLayout, LegacyFunctionHeader,
    ModernFunctionHeader, FLAG_OVERFLOWED,
};
use crate::io::ByteReader;

// When a function header is marked `FLAG_OVERFLOWED`, the inline header no
// longer holds the real field values; instead it packs the byte offset to the
// out-of-line "large" header. The two layouts pack that offset differently:
//
//   Legacy16: large_offset = (info_offset << 16) | offset
//   Modern12: large_offset = (function_name << 24) | (offset & 0x00ff_ffff)
//
// The shift amounts below name those packings.
const LEGACY_LARGE_OFFSET_SHIFT: u64 = 16;
const MODERN_LARGE_OFFSET_SHIFT: u64 = 24;
// Mask for the low 24 bits of the Modern packed offset (the `offset` portion).
const MODERN_LARGE_OFFSET_MASK: u64 = 0x00ff_ffff;

pub fn parse_function_headers(
    reader: &mut ByteReader<'_>,
    header: &BytecodeHeader,
) -> Result<Vec<FunctionHeader>> {
    let mut headers = Vec::with_capacity(reader.capacity_hint(header.function_count as usize));
    for function_id in 0..header.function_count {
        let current_pos = reader.position();
        let function_header = match header.function_header_layout {
            // Legacy Header (16 bytes):
            // Used in Hermes bytecode version < 97.
            // Compacts multiple fields into a single u128 for extreme density.
            // fields: [offset, param_count, size, name, info_offset, frame_size, env_size, registers]
            FunctionHeaderLayout::Legacy16 => {
                let raw = reader.read_bytes(16)?;
                let mut bytes = [0u8; 16];
                bytes.copy_from_slice(raw);
                let raw = u128::from_le_bytes(bytes);
                // Legacy16 bitfield map, (bit offset, width) within the 128-bit word:
                //   offset                    : ( 0, 25)
                //   param_count               : (25,  7)
                //   bytecode_size_in_bytes    : (32, 15)
                //   function_name             : (47, 17)
                //   info_offset               : (64, 25)
                //   frame_size                : (89,  7)
                //   environment_size          : (96,  8)
                //   highest_read_cache_index  : (104, 8)
                //   highest_write_cache_index : (112, 8)
                //   flags                     : (120, 8)
                let offset = (raw & ((1u128 << 25) - 1)) as u32;
                let param_count = ((raw >> 25) & ((1u128 << 7) - 1)) as u32;
                let bytecode_size_in_bytes = ((raw >> 32) & ((1u128 << 15) - 1)) as u32;
                let function_name = ((raw >> 47) & ((1u128 << 17) - 1)) as u32;
                let info_offset = ((raw >> 64) & ((1u128 << 25) - 1)) as u32;
                let frame_size = ((raw >> 89) & ((1u128 << 7) - 1)) as u32;
                let environment_size = ((raw >> 96) & 0xff) as u32;
                let highest_read_cache_index = ((raw >> 104) & 0xff) as u32;
                let highest_write_cache_index = ((raw >> 112) & 0xff) as u32;
                let flags = ((raw >> 120) & 0xff) as u8;

                if flags & FLAG_OVERFLOWED != 0 {
                    let large_offset =
                        ((info_offset as u64) << LEGACY_LARGE_OFFSET_SHIFT) | (offset as u64);
                    let large_header =
                        parse_large_header_legacy(reader, large_offset as usize, function_id)?;
                    reader.seek(current_pos + 16)?;
                    FunctionHeader::Legacy(large_header)
                } else {
                    FunctionHeader::Legacy(LegacyFunctionHeader {
                        function_id,
                        offset,
                        param_count,
                        bytecode_size_in_bytes,
                        function_name,
                        info_offset,
                        frame_size,
                        environment_size,
                        highest_read_cache_index,
                        highest_write_cache_index,
                        flags,
                    })
                }
            }
            // Modern Header (12 bytes):
            // Used in Hermes bytecode version >= 97 (including v98 and v99).
            // Even more compact (12 bytes vs 16 bytes).
            // Re-arranges bitfields for better packing and newer features (e.g., loop_depth, distinct register counts).
            // This is the default for recent React Native versions (0.75+).
            FunctionHeaderLayout::Modern12 => {
                let raw = reader.read_bytes(12)?;
                let mut bytes = [0u8; 16];
                bytes[..12].copy_from_slice(raw);
                let raw = u128::from_le_bytes(bytes);

                // Modern12 bitfield map, (bit offset, width) within the 96-bit word:
                //   offset                  : ( 0, 25)
                //   param_count             : (25,  5)
                //   loop_depth              : (30,  2)
                //   bytecode_size_in_bytes  : (32, 14)
                //   function_name           : (46,  8)
                //   number_reg_count        : (54,  5)
                //   non_ptr_reg_count       : (59,  5)
                //   frame_size              : (64,  8)
                //   read_cache_size         : (72,  8)
                //   write_cache_size        : (80,  6)   HBC 99: (80, 7)
                //   num_cache_new_object    : (86,  1)   HBC 99: absent
                //   private_name_cache_size : (87,  1)
                //   flags                   : (88,  8)
                let offset = (raw & ((1u128 << 25) - 1)) as u32;
                let param_count = ((raw >> 25) & ((1u128 << 5) - 1)) as u32;
                let loop_depth = ((raw >> 30) & ((1u128 << 2) - 1)) as u32;
                let bytecode_size_in_bytes = ((raw >> 32) & ((1u128 << 14) - 1)) as u32;
                let function_name = ((raw >> 46) & ((1u128 << 8) - 1)) as u32;
                let number_reg_count = ((raw >> 54) & ((1u128 << 5) - 1)) as u32;
                let non_ptr_reg_count = ((raw >> 59) & ((1u128 << 5) - 1)) as u32;
                let frame_size = ((raw >> 64) & 0xff) as u32;
                let read_cache_size = ((raw >> 72) & 0xff) as u8;
                // HBC 99 dropped NumCacheNewObject and gave its bit to
                // WriteCacheSize.
                let (write_cache_size, num_cache_new_object) = if header.version >= 99 {
                    (((raw >> 80) & 0x7f) as u8, 0)
                } else {
                    (((raw >> 80) & 0x3f) as u8, ((raw >> 86) & 0x1) as u8)
                };
                let private_name_cache_size = ((raw >> 87) & 0x1) as u8;
                let flags = ((raw >> 88) & 0xff) as u8;

                if flags & FLAG_OVERFLOWED != 0 {
                    let large_offset = ((function_name as u64) << MODERN_LARGE_OFFSET_SHIFT)
                        | (offset as u64 & MODERN_LARGE_OFFSET_MASK);
                    let large_header = parse_large_header_modern(
                        reader,
                        large_offset as usize,
                        function_id,
                        header.version,
                    )?;
                    reader.seek(current_pos + 12)?;
                    FunctionHeader::Modern(large_header)
                } else {
                    // Not overflowed: a 12-byte small header has no FunctionInfo
                    // section, so info_offset is 0 (no exception handlers).
                    FunctionHeader::Modern(ModernFunctionHeader {
                        function_id,
                        offset,
                        param_count,
                        loop_depth,
                        bytecode_size_in_bytes,
                        function_name,
                        number_reg_count,
                        non_ptr_reg_count,
                        frame_size,
                        read_cache_size,
                        write_cache_size,
                        num_cache_new_object,
                        private_name_cache_size,
                        flags,
                        info_offset: 0,
                    })
                }
            }
        };
        headers.push(function_header);
    }
    Ok(headers)
}

fn parse_large_header_legacy(
    reader: &mut ByteReader<'_>,
    offset: usize,
    function_id: u32,
) -> Result<LegacyFunctionHeader> {
    let current = reader.position();
    reader.seek(offset)?;
    let header = LegacyFunctionHeader {
        function_id,
        offset: reader.read_u32()?,
        param_count: reader.read_u32()?,
        bytecode_size_in_bytes: reader.read_u32()?,
        function_name: reader.read_u32()?,
        info_offset: reader.read_u32()?,
        frame_size: reader.read_u32()?,
        environment_size: reader.read_u32()?,
        highest_read_cache_index: reader.read_u8()? as u32,
        highest_write_cache_index: reader.read_u8()? as u32,
        flags: reader.read_u8()?,
    };
    reader.seek(current)?;
    Ok(header)
}

fn parse_large_header_modern(
    reader: &mut ByteReader<'_>,
    offset: usize,
    function_id: u32,
    version: u32,
) -> Result<ModernFunctionHeader> {
    let current = reader.position();
    reader.seek(offset)?;

    let mut header = ModernFunctionHeader {
        function_id,
        offset: reader.read_u32()?,
        param_count: reader.read_u32()?,
        loop_depth: reader.read_u32()?,
        bytecode_size_in_bytes: reader.read_u32()?,
        function_name: reader.read_u32()?,
        number_reg_count: reader.read_u32()?,
        non_ptr_reg_count: reader.read_u32()?,
        frame_size: reader.read_u32()?,
        read_cache_size: reader.read_u8()?,
        write_cache_size: reader.read_u8()?,
        // The large header has the same byte per cache count, and HBC 99 has
        // no NumCacheNewObject byte: 8 u32 fields, then read cache, write
        // cache, private name cache and flags, 36 bytes in all.
        num_cache_new_object: if version >= 99 { 0 } else { reader.read_u8()? },
        private_name_cache_size: reader.read_u8()?,
        flags: reader.read_u8()?,
        info_offset: 0,
    };

    // The FunctionInfo (exception handler table, then debug info) is laid out
    // immediately after the large header, 4-byte aligned. HBC >=97 small headers
    // carry no info_offset field, so a function with exception handlers / debug
    // info is emitted overflowed and its info section is located here.
    let after = reader.position();
    header.info_offset = ((after + 3) & !3) as u32;

    reader.seek(current)?;
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::BytecodeFile;
    use crate::write::create::{create_minimal, CreateOptions};

    // A real modern header to parse against, with the version under test.
    fn modern_header(version: u32) -> BytecodeHeader {
        let bytes = create_minimal(&CreateOptions {
            version: 98,
            global_body: Vec::new(),
            strings: vec!["global".into()],
        })
        .expect("create");
        let mut header = BytecodeFile::parse_auto(&bytes).expect("parse").header;
        header.version = version;
        header.function_count = 1;
        header
    }

    // A small header with every cache bit set: bits 80..86 all ones, bit 86
    // set, private name cache bit 87 set, flags 0 (not overflowed).
    fn small_header_with_cache_bits() -> [u8; 12] {
        let raw: u128 = (0x7fu128 << 80) | (1u128 << 87);
        let mut slot = [0u8; 12];
        slot.copy_from_slice(&raw.to_le_bytes()[..12]);
        slot
    }

    fn parse_small(version: u32) -> ModernFunctionHeader {
        let slot = small_header_with_cache_bits();
        let mut reader = ByteReader::new(&slot);
        let headers = parse_function_headers(&mut reader, &modern_header(version)).expect("parse");
        match &headers[0] {
            FunctionHeader::Modern(m) => m.clone(),
            other => panic!("expected a modern header, got {other:?}"),
        }
    }

    #[test]
    fn small_header_v98_splits_write_cache_and_new_object() {
        let h = parse_small(98);
        assert_eq!(h.write_cache_size, 0x3f);
        assert_eq!(h.num_cache_new_object, 1);
        assert_eq!(h.private_name_cache_size, 1);
    }

    #[test]
    fn small_header_v99_has_a_seven_bit_write_cache() {
        let h = parse_small(99);
        assert_eq!(h.write_cache_size, 0x7f);
        assert_eq!(h.num_cache_new_object, 0);
        assert_eq!(h.private_name_cache_size, 1);
    }

    // Eight u32 fields, then the cache bytes and the flags byte, followed by
    // padding up to the FunctionInfo.
    fn large_header(cache_bytes: &[u8], flags: u8) -> Vec<u8> {
        let mut buf = Vec::new();
        for v in [64u32, 2, 0, 10, 3, 4, 5, 6] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(cache_bytes);
        buf.push(flags);
        buf.resize(48, 0);
        buf
    }

    #[test]
    fn large_header_v98_reads_the_new_object_byte() {
        let buf = large_header(&[7, 8, 9, 1], 0x12);
        let h = parse_large_header_modern(&mut ByteReader::new(&buf), 0, 0, 98).expect("parse");
        assert_eq!((h.read_cache_size, h.write_cache_size), (7, 8));
        assert_eq!(h.num_cache_new_object, 9);
        assert_eq!(h.private_name_cache_size, 1);
        assert_eq!(h.flags, 0x12);
        assert_eq!(h.info_offset, 40);
    }

    // HBC 99 dropped the byte: reading it shifted the flags and put the
    // exception table one 4 byte step too far for every overflowed function.
    #[test]
    fn large_header_v99_has_no_new_object_byte() {
        let buf = large_header(&[7, 8, 1], 0x12);
        let h = parse_large_header_modern(&mut ByteReader::new(&buf), 0, 0, 99).expect("parse");
        assert_eq!(h.offset, 64);
        assert_eq!(h.frame_size, 6);
        assert_eq!((h.read_cache_size, h.write_cache_size), (7, 8));
        assert_eq!(h.num_cache_new_object, 0);
        assert_eq!(h.private_name_cache_size, 1);
        assert_eq!(h.flags, 0x12);
        assert_eq!(h.info_offset, 36);
    }
}
