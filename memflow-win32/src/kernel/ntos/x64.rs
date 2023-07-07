use std::prelude::v1::*;

use super::pehelper;
use crate::kernel::StartBlock;

use log::{debug, trace};

use memflow::architecture::{x86::x64, ArchitectureObj};
use memflow::cglue::tuple::*;
use memflow::dataview::PodMethods;
use memflow::error::{Error, ErrorKind, ErrorOrigin, PartialResultExt, Result};
use memflow::iter::PageChunks;
use memflow::mem::{MemoryView, VirtualTranslate};
use memflow::types::{mem, size, smem, umem, Address};

use pelite::image::IMAGE_DOS_HEADER;

pub fn find_with_va_hint<T: MemoryView + VirtualTranslate>(
    virt_mem: &mut T,
    start_block: &StartBlock,
) -> Result<(Address, umem)> {
    debug!(
        "x64::find_with_va_hint: trying to find ntoskrnl.exe with va hint at {:x}",
        start_block.kernel_hint.to_umem()
    );

    // va was found previously
    let mut va_base = start_block.kernel_hint.to_umem() & !0x0001_ffff;
    while va_base + mem::mb(16) > start_block.kernel_hint.to_umem() {
        trace!("x64::find_with_va_hint: probing at {:x}", va_base);

        match find_with_va(virt_mem, va_base) {
            Ok(a) => {
                let addr = Address::from(a);
                let size_of_image = pehelper::try_get_pe_size(virt_mem, addr)?;
                return Ok((addr, size_of_image));
            }
            Err(e) => trace!("x64::find_with_va_hint: probe error {:?}", e),
        }

        va_base -= mem::mb(2);
    }

    Err(Error(ErrorOrigin::OsLayer, ErrorKind::ProcessNotFound)
        .log_trace("x64::find_with_va_hint: unable to locate ntoskrnl.exe via va hint"))
}

fn find_with_va<T: MemoryView + VirtualTranslate>(virt_mem: &mut T, va_base: umem) -> Result<umem> {
    let mut buf = vec![0; size::mb(2)];
    virt_mem
        .read_raw_into(Address::from(va_base), &mut buf)
        .data_part()?;

    buf.chunks_exact(x64::ARCH.page_size())
        .enumerate()
        .map(|(i, c)| {
            let view = PodMethods::as_data_view(c);
            (i, c, view.read::<IMAGE_DOS_HEADER>(0)) // TODO: potential endian mismatch
        })
        .filter(|(_, _, p)| p.e_magic == 0x5a4d) // MZ
        .filter(|(_, _, p)| p.e_lfanew <= 0x800)
        .inspect(|(i, _, _)| {
            trace!(
                "x64::find_with_va: found potential header flags at offset {:x}",
                *i as umem * x64::ARCH.page_size() as umem
            )
        })
        .find(|(i, _, _)| {
            let probe_addr = Address::from(va_base + (*i as umem) * x64::ARCH.page_size() as umem);
            let name = pehelper::try_get_pe_name(virt_mem, probe_addr).unwrap_or_default();
            name == "ntoskrnl.exe"
        })
        .map(|(i, _, _)| va_base + i as umem * x64::ARCH.page_size() as umem)
        .ok_or_else(|| {
            Error(ErrorOrigin::OsLayer, ErrorKind::ProcessNotFound)
                .log_trace("unable to locate ntoskrnl.exe")
        })
}

pub fn find<T: MemoryView + VirtualTranslate>(
    virt_mem: &mut T,
    start_block: &StartBlock,
) -> Result<(Address, umem)> {
    debug!("x64::find: trying to find ntoskrnl.exe with page map",);

    let page_map = virt_mem.virt_page_map_range_vec(
        smem::mb(2),
        (!0u64 - (1u64 << (ArchitectureObj::from(start_block.arch).address_space_bits() - 1)))
            .into(),
        (!0u64).into(),
    );

    match page_map
        .into_iter()
        .flat_map(|CTup3(address, size, _)| size.page_chunks(address, size::mb(2)))
        .filter(|(_, size)| *size > mem::kb(256))
        .filter_map(|(va, _)| find_with_va(virt_mem, va.to_umem()).ok())
        .next()
    {
        Some(a) => {
            let addr = Address::from(a);
            let size_of_image = pehelper::try_get_pe_size(virt_mem, addr)?;
            Ok((addr, size_of_image))
        }
        None => Err(Error(ErrorOrigin::OsLayer, ErrorKind::ProcessNotFound)
            .log_trace("x64::find: unable to locate ntoskrnl.exe with a page map")),
    }
}
