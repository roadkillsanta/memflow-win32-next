use crate::kernel::{self, StartBlock};
use crate::kernel::{Win32Guid, Win32Version};

use log::{info, warn};

use memflow::architecture::ArchitectureIdent;
use memflow::cglue::forward::ForwardMut;
use memflow::error::Result;
use memflow::mem::{DirectTranslate, PhysicalMemory, VirtualDma};
use memflow::os::OsInfo;
use memflow::types::Address;

use super::Win32VirtualTranslate;

use crate::offsets::Win32OffsetBuilder;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(::serde::Serialize))]
pub struct Win32KernelInfo {
    pub os_info: OsInfo,
    pub dtb: Address,

    pub kernel_guid: Option<Win32Guid>,
    pub kernel_winver: Win32Version,

    pub eprocess_base: Address,
}

impl Win32KernelInfo {
    pub fn scanner<T: PhysicalMemory>(mem: T) -> KernelInfoScanner<T> {
        KernelInfoScanner::new(mem)
    }

    pub fn into_offset_builder<'a>(
        &self,
        mut offsets: Win32OffsetBuilder<'a>,
    ) -> Win32OffsetBuilder<'a> {
        if offsets.get_guid().is_none() && self.kernel_guid.is_some() {
            offsets = offsets.guid(self.kernel_guid.clone().unwrap());
        }

        if offsets.get_winver().is_none() {
            offsets = offsets.winver(self.kernel_winver);
        }

        if offsets.get_arch().is_none() {
            offsets = offsets.arch(self.os_info.arch.into());
        }

        offsets
    }
}

pub struct KernelInfoScanner<T> {
    mem: T,
    arch: Option<ArchitectureIdent>,
    kernel_hint: Option<Address>,
    dtb: Option<Address>,
}

impl<T: PhysicalMemory> KernelInfoScanner<T> {
    pub fn new(mem: T) -> Self {
        Self {
            mem,
            arch: None,
            kernel_hint: None,
            dtb: None,
        }
    }

    pub fn scan(mut self) -> Result<Win32KernelInfo> {
        let start_block = if let (Some(arch), Some(dtb), Some(kernel_hint)) =
            (self.arch, self.dtb, self.kernel_hint)
        {
            // construct start block from user supplied hints
            StartBlock {
                arch,
                kernel_hint,
                dtb,
            }
        } else {
            let mut sb = kernel::start_block::find(&mut self.mem, self.arch)?;
            if self.kernel_hint.is_some() && sb.kernel_hint.is_null() {
                sb.kernel_hint = self.kernel_hint.unwrap()
            }
            // dtb is always set in start_block::find()
            sb
        };

        self.scan_block(start_block).or_else(|_| {
            let start_block = kernel::start_block::find_fallback(&mut self.mem, start_block.arch)?;
            self.scan_block(start_block)
        })
    }

    fn scan_block(&mut self, start_block: StartBlock) -> Result<Win32KernelInfo> {
        info!(
            "arch={:?} kernel_hint={:x} dtb={:x}",
            start_block.arch, start_block.kernel_hint, start_block.dtb
        );

        // construct virtual memory object for start_block
        let mut virt_mem = VirtualDma::with_vat(
            self.mem.forward_mut(),
            start_block.arch,
            Win32VirtualTranslate::new(start_block.arch, start_block.dtb),
            DirectTranslate::new(),
        );

        // find ntoskrnl.exe base
        let (base, size) = kernel::ntos::find(&mut virt_mem, &start_block)?;
        info!("base={} size={}", base, size);

        // get ntoskrnl.exe guid
        let kernel_guid = kernel::ntos::find_guid(&mut virt_mem, base).ok();
        info!("kernel_guid={:?}", kernel_guid);

        let kernel_winver = kernel::ntos::find_winver(&mut virt_mem, base).ok();

        if kernel_winver.is_none() {
            warn!("Failed to retrieve kernel version! Some features may be disabled.");
        }

        let kernel_winver = kernel_winver.unwrap_or_else(|| Win32Version::new(3, 10, 511));

        info!("kernel_winver={:?}", kernel_winver);

        // find eprocess base
        let eprocess_base = kernel::sysproc::find(&mut virt_mem, &start_block, base)?;
        info!("eprocess_base={:x}", eprocess_base);

        // start_block only contains the winload's dtb which might
        // be different to the one used in the actual kernel.
        // see Kernel::new() for more information.
        info!("start_block.dtb={:x}", start_block.dtb);

        let StartBlock {
            arch,
            kernel_hint: _,
            dtb,
        } = start_block;

        Ok(Win32KernelInfo {
            os_info: OsInfo { base, size, arch },
            dtb,

            kernel_guid,
            kernel_winver,

            eprocess_base,
        })
    }

    pub fn arch(mut self, arch: ArchitectureIdent) -> Self {
        self.arch = Some(arch);
        self
    }

    pub fn kernel_hint(mut self, kernel_hint: Address) -> Self {
        self.kernel_hint = Some(kernel_hint);
        self
    }

    pub fn dtb(mut self, dtb: Address) -> Self {
        self.dtb = Some(dtb);
        self
    }
}
