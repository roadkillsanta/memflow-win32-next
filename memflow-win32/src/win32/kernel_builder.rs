use std::prelude::v1::*;

use super::{Win32Kernel, Win32KernelInfo};
use crate::offsets::Win32Offsets;

#[cfg(feature = "symstore")]
use crate::offsets::SymbolStore;

use crate::offsets::offset_builder_with_kernel_info;

use memflow::architecture::ArchitectureIdent;
use memflow::cglue::forward::ForwardMut;
use memflow::error::Result;
use memflow::mem::{
    phys_mem::CachedPhysicalMemory, virt_translate::CachedVirtualTranslate, DirectTranslate,
    PhysicalMemory, VirtualTranslate2,
};
use memflow::types::{Address, DefaultCacheValidator};

/// Builder for a Windows Kernel structure.
///
/// This function encapsulates the entire setup process for a Windows target
/// and will make sure the user gets a properly initialized object at the end.
///
/// This function is a high level abstraction over the individual parts of initialization a Windows target:
/// - Scanning for the ntoskrnl and retrieving the `Win32KernelInfo` struct.
/// - Retrieving the Offsets for the target Windows version.
/// - Creating a struct which implements `VirtualTranslate2` for virtual to physical address translations.
/// - Optionally wrapping the Connector or the `VirtualTranslate2` object into a cached object.
/// - Initialization of the Kernel structure itself.
///
/// # Examples
///
/// Using the builder with default values:
/// ```
/// use memflow::mem::PhysicalMemory;
/// use memflow_win32::win32::Win32Kernel;
///
/// fn test<T: 'static + PhysicalMemory + Clone>(connector: T) {
///     let _kernel = Win32Kernel::builder(connector)
///         .build()
///         .unwrap();
/// }
/// ```
///
/// Using the builder with default cache configurations:
/// ```
/// use memflow::mem::PhysicalMemory;
/// use memflow_win32::win32::Win32Kernel;
///
/// fn test<T: 'static + PhysicalMemory + Clone>(connector: T) {
///     let _kernel = Win32Kernel::builder(connector)
///         .build_default_caches()
///         .build()
///         .unwrap();
/// }
/// ```
///
/// Customizing the caches:
/// ```
/// use memflow::mem::{PhysicalMemory, CachedPhysicalMemory, CachedVirtualTranslate};
/// use memflow_win32::win32::Win32Kernel;
///
/// fn test<T: 'static + PhysicalMemory + Clone>(connector: T) {
///     let _kernel = Win32Kernel::builder(connector)
///     .build_page_cache(|connector, arch| {
///         CachedPhysicalMemory::builder(connector)
///             .arch(arch)
///             .build()
///             .unwrap()
///     })
///     .build_vat_cache(|vat, arch| {
///         CachedVirtualTranslate::builder(vat)
///             .arch(arch)
///             .build()
///             .unwrap()
///     })
///     .build()
///     .unwrap();
/// }
/// ```
///
/// # Remarks
///
/// Manual initialization of the above examples would look like the following:
/// ```
/// use memflow::prelude::v1::*;
/// use memflow_win32::prelude::{
///     Win32KernelInfo,
///     Win32Offsets,
///     Win32Kernel,
///     offset_builder_with_kernel_info
/// };
///
/// fn test<T: 'static + PhysicalMemory + Clone>(mut connector: T) {
///     // Use the ntoskrnl scanner to find the relevant KernelInfo (start_block, arch, dtb, ntoskrnl, etc)
///     let kernel_info = Win32KernelInfo::scanner(connector.forward_mut()).scan().unwrap();
///     // Download the corresponding pdb from the default symbol store
///     let offsets = offset_builder_with_kernel_info(&kernel_info).build().unwrap();
///
///     // Create a struct for doing virtual to physical memory translations
///     let vat = DirectTranslate::new();
///
///     // Create a Page Cache layer with default values
///     let mut connector_cached = CachedPhysicalMemory::builder(connector)
///         .arch(kernel_info.os_info.arch)
///         .build()
///         .unwrap();
///
///     // Create a Tlb Cache layer with default values
///     let vat_cached = CachedVirtualTranslate::builder(vat)
///         .arch(kernel_info.os_info.arch)
///         .build()
///         .unwrap();
///
///     // Initialize the final Kernel object
///     let _kernel = Win32Kernel::new(connector_cached, vat_cached, offsets, kernel_info);
/// }
/// ```
pub struct Win32KernelBuilder<T, TK, VK> {
    connector: T,

    arch: Option<ArchitectureIdent>,
    kernel_hint: Option<Address>,
    dtb: Option<Address>,

    #[cfg(feature = "symstore")]
    symbol_store: Option<SymbolStore>,

    build_page_cache: Box<dyn FnOnce(T, ArchitectureIdent) -> TK>,
    build_vat_cache: Box<dyn FnOnce(DirectTranslate, ArchitectureIdent) -> VK>,
}

impl<T> Win32KernelBuilder<T, T, DirectTranslate>
where
    T: PhysicalMemory,
{
    pub fn new(connector: T) -> Win32KernelBuilder<T, T, DirectTranslate> {
        Win32KernelBuilder {
            connector,

            arch: None,
            kernel_hint: None,
            dtb: None,

            #[cfg(feature = "symstore")]
            symbol_store: Some(SymbolStore::default()),

            build_page_cache: Box::new(|connector, _| connector),
            build_vat_cache: Box::new(|vat, _| vat),
        }
    }
}

impl<'a, T, TK, VK> Win32KernelBuilder<T, TK, VK>
where
    T: PhysicalMemory,
    TK: 'static + PhysicalMemory + Clone,
    VK: 'static + VirtualTranslate2 + Clone,
{
    pub fn build(mut self) -> Result<Win32Kernel<TK, VK>> {
        // find kernel_info
        let mut kernel_scanner = Win32KernelInfo::scanner(self.connector.forward_mut());
        if let Some(arch) = self.arch {
            kernel_scanner = kernel_scanner.arch(arch);
        }
        if let Some(kernel_hint) = self.kernel_hint {
            kernel_scanner = kernel_scanner.kernel_hint(kernel_hint);
        }
        if let Some(dtb) = self.dtb {
            kernel_scanner = kernel_scanner.dtb(dtb);
        }
        let kernel_info = kernel_scanner.scan()?;

        // acquire offsets from the symbol store
        let offsets = self.build_offsets(&kernel_info)?;

        // TODO: parse memory maps

        // create a vat object
        let vat = DirectTranslate::new();

        // create caches
        let kernel_connector = (self.build_page_cache)(self.connector, kernel_info.os_info.arch);
        let kernel_vat = (self.build_vat_cache)(vat, kernel_info.os_info.arch);

        // create the final kernel object
        Ok(Win32Kernel::new(
            kernel_connector,
            kernel_vat,
            offsets,
            kernel_info,
        ))
    }

    #[cfg(feature = "symstore")]
    fn build_offsets(&self, kernel_info: &Win32KernelInfo) -> Result<Win32Offsets> {
        let mut builder = offset_builder_with_kernel_info(kernel_info);
        if let Some(store) = &self.symbol_store {
            builder = builder.symbol_store(store.clone());
        } else {
            builder = builder.no_symbol_store();
        }
        builder.build()
    }

    #[cfg(not(feature = "symstore"))]
    fn build_offsets(&self, kernel_info: &Win32KernelInfo) -> Result<Win32Offsets> {
        offset_builder_with_kernel_info(&kernel_info).build()
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

    /// Configures the symbol store to be used when constructing the Kernel.
    /// This will override the default symbol store that is being used if no other setting is configured.
    ///
    /// # Examples
    ///
    /// ```
    /// use memflow::mem::PhysicalMemory;
    /// use memflow_win32::prelude::{Win32Kernel, SymbolStore};
    ///
    /// fn test<T: 'static + PhysicalMemory + Clone>(connector: T) {
    ///     let _kernel = Win32Kernel::builder(connector)
    ///         .symbol_store(SymbolStore::new().no_cache())
    ///         .build()
    ///         .unwrap();
    /// }
    /// ```
    #[cfg(feature = "symstore")]
    pub fn symbol_store(mut self, symbol_store: SymbolStore) -> Self {
        self.symbol_store = Some(symbol_store);
        self
    }

    /// Disables the symbol store when constructing the Kernel.
    /// By default a default symbol store will be used when constructing a kernel.
    /// This option allows the user to disable the symbol store alltogether
    /// and fall back to the built-in offsets table.
    ///
    /// # Examples
    ///
    /// ```
    /// use memflow::mem::PhysicalMemory;
    /// use memflow_win32::win32::Win32Kernel;
    /// use memflow_win32::offsets::SymbolStore;
    ///
    /// fn test<T: 'static + PhysicalMemory + Clone>(connector: T) {
    ///     let _kernel = Win32Kernel::builder(connector)
    ///         .no_symbol_store()
    ///         .build()
    ///         .unwrap();
    /// }
    /// ```
    #[cfg(feature = "symstore")]
    pub fn no_symbol_store(mut self) -> Self {
        self.symbol_store = None;
        self
    }

    /// Creates the Kernel structure with default caching enabled.
    ///
    /// If this option is specified, the Kernel structure is generated
    /// with a (page level cache)[../index.html] with default settings.
    /// On top of the page level cache a [vat cache](../index.html) will be setupped.
    ///
    /// # Examples
    ///
    /// ```
    /// use memflow::mem::PhysicalMemory;
    /// use memflow_win32::win32::Win32Kernel;
    ///
    /// fn test<T: 'static + PhysicalMemory + Clone>(connector: T) {
    ///     let _kernel = Win32Kernel::builder(connector)
    ///         .build_default_caches()
    ///         .build()
    ///         .unwrap();
    /// }
    /// ```
    pub fn build_default_caches(
        self,
    ) -> Win32KernelBuilder<
        T,
        CachedPhysicalMemory<'a, T, DefaultCacheValidator>,
        CachedVirtualTranslate<DirectTranslate, DefaultCacheValidator>,
    > {
        Win32KernelBuilder {
            connector: self.connector,

            arch: self.arch,
            kernel_hint: self.kernel_hint,
            dtb: self.dtb,

            #[cfg(feature = "symstore")]
            symbol_store: self.symbol_store,

            build_page_cache: Box::new(|connector, arch| {
                CachedPhysicalMemory::builder(connector)
                    .arch(arch)
                    .build()
                    .unwrap()
            }),
            build_vat_cache: Box::new(|vat, arch| {
                CachedVirtualTranslate::builder(vat)
                    .arch(arch)
                    .build()
                    .unwrap()
            }),
        }
    }

    /// Creates a Kernel structure by constructing the page cache from the given closure.
    ///
    /// This function accepts a `FnOnce` closure that is being evaluated
    /// after the ntoskrnl has been found.
    ///
    /// # Examples
    ///
    /// ```
    /// use memflow::mem::{PhysicalMemory, CachedPhysicalMemory};
    /// use memflow_win32::win32::Win32Kernel;
    ///
    /// fn test<T: 'static + PhysicalMemory + Clone>(connector: T) {
    ///     let _kernel = Win32Kernel::builder(connector)
    ///         .build_page_cache(|connector, arch| {
    ///             CachedPhysicalMemory::builder(connector)
    ///                 .arch(arch)
    ///                 .build()
    ///                 .unwrap()
    ///         })
    ///         .build()
    ///         .unwrap();
    /// }
    /// ```
    pub fn build_page_cache<TKN, F: FnOnce(T, ArchitectureIdent) -> TKN + 'static>(
        self,
        func: F,
    ) -> Win32KernelBuilder<T, TKN, VK>
    where
        TKN: PhysicalMemory,
    {
        Win32KernelBuilder {
            connector: self.connector,

            arch: self.arch,
            kernel_hint: self.kernel_hint,
            dtb: self.dtb,

            #[cfg(feature = "symstore")]
            symbol_store: self.symbol_store,

            build_page_cache: Box::new(func),
            build_vat_cache: self.build_vat_cache,
        }
    }

    /// Creates a Kernel structure by constructing the vat cache from the given closure.
    ///
    /// This function accepts a `FnOnce` closure that is being evaluated
    /// after the ntoskrnl has been found.
    ///
    /// # Examples
    ///
    /// ```
    /// use memflow::mem::{PhysicalMemory, CachedVirtualTranslate};
    /// use memflow_win32::win32::Win32Kernel;
    ///
    /// fn test<T: 'static + PhysicalMemory + Clone>(connector: T) {
    ///     let _kernel = Win32Kernel::builder(connector)
    ///         .build_vat_cache(|vat, arch| {
    ///             CachedVirtualTranslate::builder(vat)
    ///                 .arch(arch)
    ///                 .build()
    ///                 .unwrap()
    ///         })
    ///         .build()
    ///         .unwrap();
    /// }
    /// ```
    pub fn build_vat_cache<VKN, F: FnOnce(DirectTranslate, ArchitectureIdent) -> VKN + 'static>(
        self,
        func: F,
    ) -> Win32KernelBuilder<T, TK, VKN>
    where
        VKN: VirtualTranslate2,
    {
        Win32KernelBuilder {
            connector: self.connector,

            arch: self.arch,
            kernel_hint: self.kernel_hint,
            dtb: self.dtb,

            #[cfg(feature = "symstore")]
            symbol_store: self.symbol_store,

            build_page_cache: self.build_page_cache,
            build_vat_cache: Box::new(func),
        }
    }

    // TODO: more builder configurations
    // kernel_info_builder()
    // offset_builder()
}
