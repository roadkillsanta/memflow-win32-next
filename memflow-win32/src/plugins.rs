use crate::offsets::SymbolStore;
use crate::win32::{Win32Kernel, Win32KernelBuilder};

use memflow::cglue;
use memflow::plugins::{args, OsArgs};
use memflow::prelude::v1::*;
use memflow::types::cache::TimedCacheValidator;

use std::time::Duration;

#[os(name = "win32", accept_input = true, return_wrapped = true)]
pub fn create_os(
    args: &OsArgs,
    mem: Option<ConnectorInstanceArcBox<'static>>,
    lib: LibArc,
) -> Result<OsInstanceArcBox<'static>> {
    let mem = mem.ok_or_else(|| {
        Error(ErrorOrigin::OsLayer, ErrorKind::Configuration).log_error("Must provide memory!")
    })?;

    let builder = Win32Kernel::builder(mem);
    build_dtb(builder, &args.extra_args, lib)
}

fn build_final<
    A: 'static + PhysicalMemory + Clone,
    B: 'static + PhysicalMemory + Clone,
    C: 'static + VirtualTranslate2 + Clone,
>(
    kernel_builder: Win32KernelBuilder<A, B, C>,
    _: &Args,
    lib: LibArc,
) -> Result<OsInstanceArcBox<'static>> {
    log::info!(
        "Building kernel of type {}",
        std::any::type_name::<Win32KernelBuilder<A, B, C>>()
    );
    let kernel = kernel_builder.build()?;
    Ok(group_obj!((kernel, lib) as OsInstance))
}

fn build_arch<
    A: 'static + PhysicalMemory + Clone,
    B: 'static + PhysicalMemory + Clone,
    C: 'static + VirtualTranslate2 + Clone,
>(
    builder: Win32KernelBuilder<A, B, C>,
    args: &Args,
    lib: LibArc,
) -> Result<OsInstanceArcBox<'static>> {
    match args.get("arch").map(|a| a.to_lowercase()).as_deref() {
        Some("x64") => build_final(builder.arch(ArchitectureIdent::X86(64, false)), args, lib),
        Some("x32") => build_final(builder.arch(ArchitectureIdent::X86(32, false)), args, lib),
        Some("x32_pae") => build_final(builder.arch(ArchitectureIdent::X86(32, true)), args, lib),
        Some("aarch64") => build_final(
            builder.arch(ArchitectureIdent::AArch64(size::kb(4))),
            args,
            lib,
        ),
        _ => build_final(builder, args, lib),
    }
}

fn build_symstore<
    A: 'static + PhysicalMemory + Clone,
    B: 'static + PhysicalMemory + Clone,
    C: 'static + VirtualTranslate2 + Clone,
>(
    builder: Win32KernelBuilder<A, B, C>,
    args: &Args,
    lib: LibArc,
) -> Result<OsInstanceArcBox<'static>> {
    match args.get("symstore") {
        Some("uncached") => build_arch(
            builder.symbol_store(SymbolStore::new().no_cache()),
            args,
            lib,
        ),
        Some("none") => build_arch(builder.no_symbol_store(), args, lib),
        _ => build_arch(builder, args, lib),
    }
}

fn build_kernel_hint<
    A: 'static + PhysicalMemory + Clone,
    B: 'static + PhysicalMemory + Clone,
    C: 'static + VirtualTranslate2 + Clone,
>(
    builder: Win32KernelBuilder<A, B, C>,
    args: &Args,
    lib: LibArc,
) -> Result<OsInstanceArcBox<'static>> {
    match args
        .get("kernel_hint")
        .and_then(|d| u64::from_str_radix(d, 16).ok())
    {
        Some(dtb) => build_symstore(builder.kernel_hint(Address::from(dtb)), args, lib),
        _ => build_symstore(builder, args, lib),
    }
}

fn build_vat<
    A: 'static + PhysicalMemory + Clone,
    B: 'static + PhysicalMemory + Clone,
    C: 'static + VirtualTranslate2 + Clone,
>(
    builder: Win32KernelBuilder<A, B, C>,
    args: &Args,
    lib: LibArc,
) -> Result<OsInstanceArcBox<'static>> {
    match args::parse_vatcache(args)? {
        Some((0, _)) => build_kernel_hint(
            builder.build_vat_cache(|v, a| {
                CachedVirtualTranslate::builder(v).arch(a).build().unwrap()
            }),
            args,
            lib,
        ),
        Some((size, time)) => build_kernel_hint(
            builder.build_vat_cache(move |v, a| {
                let builder = CachedVirtualTranslate::builder(v).arch(a).entries(size);

                if time > 0 {
                    builder
                        .validator(TimedCacheValidator::new(Duration::from_millis(time).into()))
                        .build()
                        .unwrap()
                } else {
                    builder.build().unwrap()
                }
            }),
            args,
            lib,
        ),
        None => build_kernel_hint(builder, args, lib),
    }
}

fn build_dtb<
    A: 'static + PhysicalMemory + Clone,
    B: 'static + PhysicalMemory + Clone,
    C: 'static + VirtualTranslate2 + Clone,
>(
    builder: Win32KernelBuilder<A, B, C>,
    args: &Args,
    lib: LibArc,
) -> Result<OsInstanceArcBox<'static>> {
    match args
        .get("dtb")
        .and_then(|d| u64::from_str_radix(d, 16).ok())
    {
        Some(dtb) => build_vat(builder.dtb(Address::from(dtb)), args, lib),
        _ => build_vat(builder, args, lib),
    }
}
