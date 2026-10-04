//! The only Rust unsafe boundary. Compiler Reply buffers use the C++ bridge's
//! malloc/free pair; runtime ABI buffers use this crate's Rust Box allocator
//! and its separate versioned release function. The owners never interchange.
#![allow(unsafe_code)]

#[cfg(feature = "embedded")]
mod compiler {
    use std::ffi::{CString, c_char};
    use std::sync::Mutex;

    use crate::{BinaryResource, Target};

    const MAX_REPLY_BYTES: usize = 320 * 1024 * 1024;

    #[repr(C)]
    struct Resource {
        symbol: *const u8,
        symbol_len: usize,
        bytes: *const u8,
        len: usize,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Reply {
        bytes: *mut u8,
        len: usize,
        status: u32,
    }

    unsafe extern "C" {
        fn dever_backend_object(
            ir: *const u8,
            len: usize,
            triple: *const c_char,
            resources: *const Resource,
            resource_count: usize,
            reply: *mut Reply,
        );
        fn dever_backend_link(args: *const *const c_char, count: usize, reply: *mut Reply);
        fn dever_backend_free(bytes: *mut u8);
    }

    impl Drop for Reply {
        fn drop(&mut self) {
            // SAFETY: only our C++ bridge assigns bytes, by malloc; NULL is valid.
            // The buffer is freed once, after all reads, by its owning allocator.
            unsafe { dever_backend_free(self.bytes) };
        }
    }

    impl Reply {
        fn take(&self) -> Result<Vec<u8>, String> {
            if self.len > MAX_REPLY_BYTES || (self.len != 0 && self.bytes.is_null()) {
                return Err("compiler bridge returned an invalid buffer".into());
            }
            let bytes = if self.len == 0 {
                &[]
            } else {
                // SAFETY: the bridge returns a live allocation of exactly len bytes.
                // Reply owns it until Drop; no foreign mutation occurs after return.
                unsafe { std::slice::from_raw_parts(self.bytes, self.len) }
            };
            if self.status != 0 && bytes.is_empty() {
                return Err("compiler bridge could not allocate a diagnostic".into());
            }
            if self.status == 0 {
                Ok(bytes.to_vec())
            } else {
                Err(String::from_utf8_lossy(bytes).into_owned())
            }
        }
    }

    pub(crate) fn emit_object(
        ir: &str,
        target: Target,
        resources: &[BinaryResource<'_>],
    ) -> Result<Vec<u8>, String> {
        let triple = CString::new(target.triple()).expect("static target contains no NUL");
        let resources = resources
            .iter()
            .map(|resource| Resource {
                symbol: resource.symbol.as_ptr(),
                symbol_len: resource.symbol.len(),
                bytes: resource.bytes.as_ptr(),
                len: resource.bytes.len(),
            })
            .collect::<Vec<_>>();
        let mut reply = Reply::default();
        // SAFETY: IR and NUL-terminated triple outlive this synchronous call.
        // C++ reads the validated borrowed slices only during this call and copies
        // their bytes into LLVM-owned constants before returning its owned reply.
        unsafe {
            dever_backend_object(
                ir.as_ptr(),
                ir.len(),
                triple.as_ptr(),
                resources.as_ptr(),
                resources.len(),
                &mut reply,
            )
        };
        reply.take()
    }

    // LLD's global context cannot be used concurrently. The C++ owner terminates
    // the compiler worker rather than returning an unrecoverable linker context.
    static LLD_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn link(args: &[CString]) -> Result<(), String> {
        let _guard = LLD_LOCK.lock().map_err(|_| "LLD state is poisoned")?;
        let pointers = args.iter().map(|arg| arg.as_ptr()).collect::<Vec<_>>();
        let mut reply = Reply::default();
        // SAFETY: each CString, the pointer array and reply live through the call.
        // The mutex serializes calls; C++ returns only after safe LLD recovery.
        unsafe { dever_backend_link(pointers.as_ptr(), pointers.len(), &mut reply) };
        reply.take().map(|_| ())
    }
}

#[cfg(feature = "embedded")]
pub(super) use compiler::{emit_object, link};

#[cfg(feature = "runtime-abi")]
mod runtime {
    use std::ptr;

    use dever_runtime::{abi, number, time};

    use crate::{ABI_INVALID_INPUT, ABI_OK, ABI_RUNTIME_ERROR, AbiDecimal};

    /// # Safety
    /// Call only from the generated process main, before starting application
    /// threads. Callable roots must preserve the embedding host's signal policy.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_process_init() -> u32 {
        // The CRT enters our C main without Rust's lang_start initialization.
        // Preserve the Rust backend's EPIPE error/cleanup path on Unix.
        #[cfg(unix)]
        if unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) } == libc::SIG_ERR {
            return ABI_RUNTIME_ERROR;
        }
        ABI_OK
    }

    /// # Safety
    /// `bytes` borrows `len` readable bytes until return; null is valid for zero length.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_process_stderr(bytes: *const u8, len: u64) -> u32 {
        use std::io::Write;

        if len > isize::MAX as u64 || (len != 0 && bytes.is_null()) {
            return ABI_INVALID_INPUT;
        }
        let bytes = if len == 0 {
            &[]
        } else {
            // SAFETY: the process entry borrows a live text/buffer allocation.
            unsafe { std::slice::from_raw_parts(bytes, len as usize) }
        };
        let mut stderr = std::io::stderr().lock();
        match stderr.write_all(bytes).and_then(|()| stderr.flush()) {
            Ok(()) => ABI_OK,
            Err(_) => ABI_RUNTIME_ERROR,
        }
    }

    /// # Safety
    /// `argv` borrows the CRT's argc pointers to NUL-terminated arguments.
    /// `out` is a distinct writable u64 slot, initialized only on success.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_process_test_index(
        argc: i32,
        argv: *const *const std::ffi::c_char,
        count: u64,
        out: *mut u64,
    ) -> u32 {
        if argc != 2 || argv.is_null() || !argv.is_aligned() || out.is_null() || !out.is_aligned() {
            return ABI_INVALID_INPUT;
        }
        // SAFETY: argc=2 and the caller supplies the live CRT argument array.
        let argument = unsafe { *argv.add(1) };
        if argument.is_null() {
            return ABI_INVALID_INPUT;
        }
        // SAFETY: each CRT argument is NUL terminated and borrowed during main.
        let digits = unsafe { std::ffi::CStr::from_ptr(argument) }.to_bytes();
        if digits.is_empty() {
            return ABI_INVALID_INPUT;
        }
        let index = digits.iter().try_fold(0_u64, |value, digit| {
            if !digit.is_ascii_digit() {
                return None;
            }
            value.checked_mul(10)?.checked_add(u64::from(*digit - b'0'))
        });
        match index {
            Some(index) if index < count => {
                // SAFETY: the caller supplied a writable, aligned output slot.
                unsafe { out.write(index) };
                ABI_OK
            }
            _ => ABI_INVALID_INPUT,
        }
    }

    mod asynchronous {
        use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
        use std::ffi::c_void;
        use std::future::Future;
        use std::pin::Pin;
        use std::ptr::{self, NonNull};
        use std::sync::{Arc, Mutex};
        use std::task::{Context, Poll};

        use dever_runtime::async_stream::AsyncStream;
        use dever_runtime::bytes::Bytes;
        use dever_runtime::channel::Channel;
        use dever_runtime::{net, task};

        use super::managed::{self, Element};
        use super::{Buffer, empty, error};
        use crate::ABI_INVALID_INPUT;

        type MoveValue = unsafe extern "C" fn(*mut c_void, *mut c_void);
        type DropValue = unsafe extern "C" fn(*mut c_void);

        #[repr(C)]
        pub(super) struct OwnedType {
            size: u64,
            align: u64,
            move_init: Option<MoveValue>,
            drop: Option<DropValue>,
            transferable: u8,
        }

        #[repr(C)]
        pub(super) struct AsyncFunction {
            output_type: *const OwnedType,
            fault_type: *const OwnedType,
            create: Option<
                unsafe extern "C" fn(
                    *mut PollState,
                    *const c_void,
                    *mut c_void,
                    *mut c_void,
                ) -> *mut c_void,
            >,
            resume: Option<unsafe extern "C" fn(*mut c_void)>,
            destroy: Option<unsafe extern "C" fn(*mut c_void)>,
            done: Option<unsafe extern "C" fn(*mut c_void) -> u8>,
            send_safe: u8,
        }

        #[repr(C)]
        pub(super) struct SyncFunction {
            input_type: *const OwnedType,
            output_type: *const OwnedType,
            fault_type: *const OwnedType,
            invoke: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> u32>,
            send_safe: u8,
        }

        struct PreparedSync {
            input: Owned,
            output: Owned,
            fault: Owned,
            invoke: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> u32,
            unit_output: bool,
        }

        impl PreparedSync {
            fn execute(self) -> Result<Owned, ForeignFault> {
                let Self {
                    input,
                    mut output,
                    mut fault,
                    invoke,
                    ..
                } = self;
                // SAFETY: the checked synchronous callback receives one
                // initialized input and matching uninitialized result slots.
                let status = unsafe { invoke(input.pointer(), output.pointer(), fault.pointer()) };
                match status {
                    0 => {
                        output.initialized = true;
                        Ok(output)
                    }
                    1 => {
                        fault.initialized = true;
                        Err(ForeignFault::Typed(fault))
                    }
                    _ => Err(ForeignFault::from(
                        "synchronous function returned invalid status".to_owned(),
                    )),
                }
            }
        }

        struct Owned {
            pointer: NonNull<u8>,
            layout: Layout,
            ty: &'static OwnedType,
            initialized: bool,
        }

        // Compiler descriptors are process-static and generated only for
        // checker-approved transferable values before spawning child tasks.
        unsafe impl Send for Owned {}

        unsafe fn owned_layout(
            ty: *const OwnedType,
        ) -> Result<(&'static OwnedType, Layout), String> {
            if ty.is_null() {
                return Err("missing async type descriptor".into());
            }
            if !ty.is_aligned() {
                return Err("unaligned async type descriptor".into());
            }
            let ty = unsafe { &*ty };
            let size = usize::try_from(ty.size).map_err(|_| "invalid async value size")?;
            let align = usize::try_from(ty.align).map_err(|_| "invalid async value alignment")?;
            let layout = Layout::from_size_align(size.max(1), align)
                .map_err(|_| "invalid async value layout")?;
            if ty.move_init.is_none() || ty.drop.is_none() {
                return Err("invalid async value descriptor".into());
            }
            Ok((ty, layout))
        }

        impl Owned {
            unsafe fn new(ty: *const OwnedType) -> Result<Self, String> {
                let (ty, layout) = unsafe { owned_layout(ty) }?;
                let pointer =
                    NonNull::new(unsafe { alloc(layout) }).ok_or("cannot allocate async value")?;
                Ok(Self {
                    pointer,
                    layout,
                    ty,
                    initialized: false,
                })
            }

            fn pointer(&self) -> *mut c_void {
                self.pointer.as_ptr().cast()
            }

            unsafe fn take_from(&mut self, source: *mut c_void) {
                // SAFETY: the callback moves one initialized value into this
                // uninitialized owner, and the caller disarms its old slot.
                unsafe {
                    (self.ty.move_init.expect("validated descriptor"))(source, self.pointer())
                };
                self.initialized = true;
            }

            unsafe fn move_to(&mut self, destination: *mut c_void) {
                debug_assert!(self.initialized);
                // SAFETY: the compiler callback moves one initialized concrete
                // value into its matching aligned, uninitialized destination.
                unsafe {
                    (self.ty.move_init.expect("validated descriptor"))(self.pointer(), destination)
                };
                self.initialized = false;
            }
        }

        impl Drop for Owned {
            fn drop(&mut self) {
                if self.initialized {
                    // SAFETY: this owner has not transferred its concrete value.
                    unsafe { (self.ty.drop.expect("validated descriptor"))(self.pointer()) };
                }
                // SAFETY: pointer was allocated with this exact layout.
                unsafe { dealloc(self.pointer.as_ptr(), self.layout) };
            }
        }

        enum ForeignFault {
            Typed(Owned),
            Runtime(String),
        }

        impl From<String> for ForeignFault {
            fn from(message: String) -> Self {
                Self::Runtime(message)
            }
        }

        pub(super) struct PollState {
            current: *mut c_void,
            completion: u32,
        }

        struct ForeignFuture {
            state: Box<PollState>,
            frame: NonNull<c_void>,
            output: Option<Owned>,
            fault: Option<Owned>,
            resume: unsafe extern "C" fn(*mut c_void),
            destroy: unsafe extern "C" fn(*mut c_void),
            done: unsafe extern "C" fn(*mut c_void) -> u8,
        }

        struct SendForeignFuture(ForeignFuture);

        // Only task/group spawn constructs this wrapper, after checking the
        // whole child frame and both result descriptors for transferability.
        unsafe impl Send for SendForeignFuture {}

        impl Future for SendForeignFuture {
            type Output = Result<Owned, ForeignFault>;

            fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
                Pin::new(&mut self.get_mut().0).poll(context)
            }
        }

        impl ForeignFuture {
            unsafe fn new(
                function: *const AsyncFunction,
                input: *const c_void,
            ) -> Result<Self, String> {
                let function =
                    unsafe { function.as_ref() }.ok_or("missing async function descriptor")?;
                let mut output = unsafe { Owned::new(function.output_type) }?;
                let mut fault = unsafe { Owned::new(function.fault_type) }?;
                let create = function.create.ok_or("missing async constructor")?;
                let resume = function.resume.ok_or("missing async resume callback")?;
                let destroy = function.destroy.ok_or("missing async destroy callback")?;
                let done = function.done.ok_or("missing async completion callback")?;
                let mut state = Box::new(PollState {
                    current: ptr::null_mut(),
                    completion: 0,
                });
                // SAFETY: the callback's process-static ABI clones borrowed
                // inputs and initializes its owned frame before returning.
                let frame =
                    unsafe { create(&mut *state, input, output.pointer(), fault.pointer()) };
                let frame = NonNull::new(frame).ok_or("cannot create async frame")?;
                output.initialized = false;
                fault.initialized = false;
                Ok(Self {
                    state,
                    frame,
                    output: Some(output),
                    fault: Some(fault),
                    resume,
                    destroy,
                    done,
                })
            }
        }

        impl Future for ForeignFuture {
            type Output = Result<Owned, ForeignFault>;

            fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
                let this = self.get_mut();
                this.state.current = (context as *mut Context<'_>).cast();
                // SAFETY: only this poll owns the frame, and current is live.
                unsafe { (this.resume)(this.frame.as_ptr()) };
                this.state.current = ptr::null_mut();
                if unsafe { (this.done)(this.frame.as_ptr()) } == 0 {
                    return Poll::Pending;
                }
                match this.state.completion {
                    1 => {
                        let mut output = this.output.take().expect("async output once");
                        output.initialized = true;
                        Poll::Ready(Ok(output))
                    }
                    2 => {
                        let mut fault = this.fault.take().expect("async fault once");
                        fault.initialized = true;
                        Poll::Ready(Err(ForeignFault::Typed(fault)))
                    }
                    _ => Poll::Ready(Err(ForeignFault::Runtime(
                        "async frame completed without a result".into(),
                    ))),
                }
            }
        }

        impl Drop for ForeignFuture {
            fn drop(&mut self) {
                // SAFETY: destroy runs once, including at the initial and final
                // suspend points, then releases the frame's allocator owner.
                unsafe { (self.destroy)(self.frame.as_ptr()) };
            }
        }

        pub(super) struct AsyncOp {
            future: Pin<Box<dyn Future<Output = Result<OpValue, ForeignFault>>>>,
            completed: bool,
            output_slots: Option<OutputSlots>,
        }

        #[derive(Clone, Copy)]
        enum OutputSlots {
            Unit,
            Bool,
            #[cfg(all(
                feature = "runtime-api",
                any(feature = "runtime-sqlite", feature = "runtime-postgres")
            ))]
            Int,
            Handle,
            OptionalHandle,
            #[cfg(feature = "runtime-api")]
            Typed {
                align: usize,
                fault_align: usize,
            },
            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            Database {
                align: usize,
                optional: bool,
                fault_align: usize,
            },
        }

        impl OutputSlots {
            fn validate(
                self,
                output: *mut c_void,
                present: *mut u8,
                _fault: *mut c_void,
            ) -> Result<(), &'static str> {
                match self {
                    #[cfg(feature = "runtime-api")]
                    Self::Typed { align, fault_align } => {
                        if output.is_null()
                            || !(output as usize).is_multiple_of(align)
                            || _fault.is_null()
                            || !(_fault as usize).is_multiple_of(fault_align)
                        {
                            return Err("invalid application typed output");
                        }
                        Ok(())
                    }
                    #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
                    Self::Database {
                        align,
                        optional,
                        fault_align,
                    } => {
                        if (align != 0
                            && (output.is_null() || !(output as usize).is_multiple_of(align)))
                            || (optional && present.is_null())
                            || _fault.is_null()
                            || !(_fault as usize).is_multiple_of(fault_align)
                        {
                            return Err("invalid database operation output");
                        }
                        Ok(())
                    }
                    Self::Unit => Ok(()),
                    #[cfg(all(
                        feature = "runtime-api",
                        any(feature = "runtime-sqlite", feature = "runtime-postgres")
                    ))]
                    Self::Int => {
                        if output.is_null() || !output.cast::<i64>().is_aligned() {
                            Err("invalid Job count output")
                        } else {
                            Ok(())
                        }
                    }
                    Self::Bool if output.is_null() => Err("missing protocol bool output"),
                    Self::Bool => Ok(()),
                    Self::Handle | Self::OptionalHandle => {
                        if output.is_null() || !output.cast::<*mut c_void>().is_aligned() {
                            return Err("invalid protocol handle output");
                        }
                        if matches!(self, Self::OptionalHandle) && present.is_null() {
                            return Err("missing message presence output");
                        }
                        Ok(())
                    }
                }
            }
        }

        enum TaskKind {
            Value(task::Task<Owned, ForeignFault>, &'static OwnedType),
            Unit(task::Task<(), ForeignFault>),
        }

        impl TaskKind {
            fn output_type(&self) -> Option<&'static OwnedType> {
                match self {
                    Self::Value(_, ty) => Some(ty),
                    Self::Unit(_) => None,
                }
            }
        }

        pub(super) struct TaskHandle(Arc<Mutex<Option<TaskKind>>>);

        fn take_task(handle: TaskHandle) -> Result<TaskKind, ForeignFault> {
            handle
                .0
                .lock()
                .map_err(|_| ForeignFault::from("async task state lock failed".to_owned()))?
                .take()
                .ok_or_else(|| ForeignFault::from("async task was already consumed".to_owned()))
        }

        pub(super) struct GroupHandle(Arc<Mutex<Option<task::Group<ForeignFault>>>>);
        pub(super) struct ChannelHandle(Channel<Owned>);

        enum StreamRow {
            Managed(Element),
            Moved(Owned),
            Int(i64),
        }

        struct AsyncStreamHandle(AsyncStream<StreamRow>);

        impl StreamRow {
            fn pointer(&self) -> *const c_void {
                match self {
                    Self::Managed(value) => value.pointer(),
                    Self::Moved(value) => value.pointer(),
                    Self::Int(value) => std::ptr::from_ref(value).cast(),
                }
            }
        }

        type PackInput = unsafe extern "C" fn(*const c_void, *const c_void, *mut c_void);

        #[repr(C)]
        pub(super) struct ParallelFunction {
            asynchronous: *const AsyncFunction,
            synchronous: *const SyncFunction,
            input_type: *const OwnedType,
            context_type: *const managed::TypeDescriptor,
            pack: Option<PackInput>,
        }

        struct ParallelHandler {
            asynchronous: Option<&'static AsyncFunction>,
            synchronous: Option<&'static SyncFunction>,
            input_type: &'static OwnedType,
            context: Option<Element>,
            pack: PackInput,
        }

        // Only new constructs this owner, after checking compiler Send metadata
        // for the complete packed inputs, child frame, outputs and typed faults.
        // The descriptors are immutable process-static data; context is borrowed
        // immutably by pack, which creates a distinct owned input for every child.
        unsafe impl Send for ParallelHandler {}
        unsafe impl Sync for ParallelHandler {}

        impl ParallelHandler {
            unsafe fn new(
                function: *const ParallelFunction,
                context: *const c_void,
            ) -> Result<Self, String> {
                let function = unsafe { function.as_ref() }.ok_or("missing parallel function")?;
                let input_type =
                    unsafe { function.input_type.as_ref() }.ok_or("missing parallel input type")?;
                let asynchronous = unsafe { function.asynchronous.as_ref() };
                let synchronous = unsafe { function.synchronous.as_ref() };
                let (send_safe, output_type, fault_type) = match (asynchronous, synchronous) {
                    (Some(function), None) => (
                        function.send_safe,
                        function.output_type,
                        function.fault_type,
                    ),
                    (None, Some(function)) if std::ptr::eq(function.input_type, input_type) => (
                        function.send_safe,
                        function.output_type,
                        function.fault_type,
                    ),
                    _ => {
                        return Err(
                            "parallel function requires exactly one matching handler".into()
                        );
                    }
                };
                for ty in [function.input_type, output_type, fault_type] {
                    let (ty, _) = unsafe { owned_layout(ty) }?;
                    if send_safe == 0 || ty.transferable == 0 {
                        return Err("parallel handler is not transferable".into());
                    }
                }
                if unsafe { &*output_type }.size != 0 {
                    return Err("parallel handler must return Unit".into());
                }
                if asynchronous.is_some_and(|function| {
                    function.create.is_none()
                        || function.resume.is_none()
                        || function.destroy.is_none()
                        || function.done.is_none()
                }) || synchronous.is_some_and(|function| function.invoke.is_none())
                {
                    return Err("incomplete parallel handler".into());
                }
                let pack = function
                    .pack
                    .ok_or("missing parallel input pack callback")?;
                let context = if function.context_type.is_null() {
                    None
                } else {
                    let ops = unsafe { managed::descriptor(function.context_type, false) }
                        .map_err(|failure| failure.message)?;
                    Some(
                        unsafe { Element::from_source(ops, context) }
                            .map_err(|failure| failure.message)?,
                    )
                };
                Ok(Self {
                    asynchronous,
                    synchronous,
                    input_type,
                    context,
                    pack,
                })
            }

            fn input(&self, row: &StreamRow) -> Result<Owned, ForeignFault> {
                let mut input =
                    unsafe { Owned::new(self.input_type) }.map_err(ForeignFault::from)?;
                let context = self.context.as_ref().map_or(ptr::null(), Element::pointer);
                // SAFETY: pack initializes the matching concrete input type from
                // borrowed element/context. The compiler verified their types.
                unsafe { (self.pack)(row.pointer(), context, input.pointer()) };
                input.initialized = true;
                Ok(input)
            }

            fn execute(&self, row: StreamRow) -> Result<(), ForeignFault> {
                let function = self.synchronous.expect("validated synchronous dispatch");
                let input = self.input(&row)?;
                let prepared = PreparedSync {
                    input,
                    output: unsafe { Owned::new(function.output_type) }
                        .map_err(ForeignFault::from)?,
                    fault: unsafe { Owned::new(function.fault_type) }
                        .map_err(ForeignFault::from)?,
                    invoke: function.invoke.expect("validated handler"),
                    unit_output: true,
                };
                prepared.execute().map(drop)
            }

            fn start(&self, row: StreamRow) -> Result<SendForeignFuture, ForeignFault> {
                let input = self.input(&row)?;
                let function = self.asynchronous.expect("validated async dispatch");
                // ForeignFuture::new clones this borrowed input into the frame
                // before initial suspend, so this temporary may now be dropped.
                unsafe { ForeignFuture::new(function, input.pointer()) }
                    .map(SendForeignFuture)
                    .map_err(ForeignFault::from)
            }
        }

        struct GroupLease {
            shared: Arc<Mutex<Option<task::Group<ForeignFault>>>>,
            group: Option<task::Group<ForeignFault>>,
        }

        impl GroupLease {
            fn take(
                shared: Arc<Mutex<Option<task::Group<ForeignFault>>>>,
            ) -> Result<Self, ForeignFault> {
                let group = shared
                    .lock()
                    .map_err(|_| ForeignFault::from("async group state lock failed".to_owned()))?
                    .take()
                    .ok_or_else(|| {
                        ForeignFault::from("async group is already in use".to_owned())
                    })?;
                Ok(Self {
                    shared,
                    group: Some(group),
                })
            }
        }

        impl Drop for GroupLease {
            fn drop(&mut self) {
                if let Ok(mut slot) = self.shared.lock() {
                    *slot = self.group.take();
                }
            }
        }

        enum OpValue {
            Unit,
            Value(Owned),
            OptionalValue(Option<Owned>),
            Task(TaskKind),
            StreamRow(Option<StreamRow>),
            Socket(net::Socket),
            Listener(net::Listener),
            Bytes(Option<Bytes>),
            Bool(bool),
            OptionalUnit(bool),
            Protocol(protocol::Value),
            #[cfg(feature = "runtime-external")]
            External(dever_runtime::component::Reply),
            #[cfg(feature = "runtime-api")]
            Api(api_application::Output),
            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            Database(database::Output),
        }

        fn operation(
            future: impl Future<Output = Result<OpValue, ForeignFault>> + 'static,
        ) -> *mut AsyncOp {
            operation_with_output(future, None)
        }

        fn operation_with_output(
            future: impl Future<Output = Result<OpValue, ForeignFault>> + 'static,
            output_slots: Option<OutputSlots>,
        ) -> *mut AsyncOp {
            Box::into_raw(Box::new(AsyncOp {
                future: Box::pin(future),
                completed: false,
                output_slots,
            }))
        }

        /// # Safety
        /// The descriptor and callbacks are process-static compiler output;
        /// input remains borrowed through create; output, fault and error are
        /// distinct writable slots of their declared concrete types.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_root(
            function: *const AsyncFunction,
            input: *const c_void,
            output: *mut c_void,
            fault: *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if !unsafe { empty(buffer) } || output.is_null() || fault.is_null() {
                return 4;
            }
            let future = match unsafe { ForeignFuture::new(function, input) } {
                Ok(future) => future,
                Err(message) => return unsafe { error(buffer, 4, message) },
            };
            let result = task::run_entry_with_typed(task::RuntimeConfig::default(), future);
            unsafe { root_result(result, output, fault, buffer) }
        }

        unsafe fn root_result(
            result: Result<Owned, ForeignFault>,
            output: *mut c_void,
            fault: *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            match result {
                Ok(mut value) => {
                    // SAFETY: caller supplied the matching uninitialized slot.
                    unsafe { value.move_to(output) };
                    1
                }
                Err(ForeignFault::Typed(mut value)) => {
                    // SAFETY: caller supplied the matching uninitialized slot.
                    unsafe { value.move_to(fault) };
                    2
                }
                Err(ForeignFault::Runtime(message)) => unsafe { error(buffer, 3, message) },
            }
        }

        #[cfg(any(
            feature = "runtime-external",
            feature = "runtime-api",
            feature = "runtime-sqlite",
            feature = "runtime-postgres"
        ))]
        fn cleanup_result(
            result: Result<Owned, ForeignFault>,
            cleanup: Result<(), String>,
            append: unsafe extern "C" fn(*mut c_void, *const c_void),
        ) -> Result<Owned, ForeignFault> {
            match (result, cleanup) {
                (result, Ok(())) => result,
                (Ok(_), Err(message)) => Err(ForeignFault::Runtime(message)),
                (Err(ForeignFault::Runtime(primary)), Err(message)) => Err(ForeignFault::Runtime(
                    format!("{primary}; cleanup: {message}"),
                )),
                (Err(ForeignFault::Typed(primary)), Err(message)) => {
                    let message = managed::owned(message);
                    // SAFETY: the root validated this callback for its concrete fault.
                    unsafe {
                        append(primary.pointer(), message);
                        managed::release::<String>(message);
                    }
                    Err(ForeignFault::Typed(primary))
                }
            }
        }

        /// # Safety
        /// `state` is the live frame owner supplied to the constructor.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_complete(state: *mut PollState, status: u32) {
            if let Some(state) = unsafe { state.as_mut() } {
                state.completion = status;
            }
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn dever_rt_v1_async_sleep(milliseconds: i64) -> *mut AsyncOp {
            operation(async move {
                task::sleep(milliseconds)
                    .await
                    .map_err(|message| ForeignFault::Runtime(message.to_owned()))?;
                Ok(OpValue::Unit)
            })
        }

        /// # Safety
        /// The process-static descriptor borrows input only through create.
        /// A same-scope call can poll a non-Send frame on the root thread;
        /// child spawning applies the separate send_safe check.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_call(
            function: *const AsyncFunction,
            input: *const c_void,
            buffer: *mut Buffer,
        ) -> *mut AsyncOp {
            if !unsafe { empty(buffer) } {
                return ptr::null_mut();
            }
            let future = match unsafe { ForeignFuture::new(function, input) } {
                Ok(future) => future,
                Err(message) => {
                    unsafe { error(buffer, ABI_INVALID_INPUT, message) };
                    return ptr::null_mut();
                }
            };
            operation(async move { future.await.map(OpValue::Value) })
        }

        unsafe fn prepare_sync(
            function: *const SyncFunction,
            input: *mut c_void,
            buffer: *mut Buffer,
        ) -> Option<PreparedSync> {
            if !unsafe { empty(buffer) } {
                return None;
            }
            let Some(function) = (unsafe { function.as_ref() }) else {
                unsafe {
                    error(
                        buffer,
                        ABI_INVALID_INPUT,
                        "missing synchronous function descriptor",
                    )
                };
                return None;
            };
            let (Some(input_type), Some(output_type), Some(fault_type), Some(invoke)) = (
                unsafe { function.input_type.as_ref() },
                unsafe { function.output_type.as_ref() },
                unsafe { function.fault_type.as_ref() },
                function.invoke,
            ) else {
                unsafe {
                    error(
                        buffer,
                        ABI_INVALID_INPUT,
                        "incomplete synchronous function descriptor",
                    )
                };
                return None;
            };
            if input.is_null()
                || function.send_safe == 0
                || input_type.transferable == 0
                || output_type.transferable == 0
                || fault_type.transferable == 0
            {
                unsafe {
                    error(
                        buffer,
                        ABI_INVALID_INPUT,
                        "blocking function value is not transferable",
                    )
                };
                return None;
            }
            let mut input_owner = match unsafe { Owned::new(function.input_type) } {
                Ok(value) => value,
                Err(message) => {
                    unsafe { error(buffer, ABI_INVALID_INPUT, message) };
                    return None;
                }
            };
            let output = match unsafe { Owned::new(function.output_type) } {
                Ok(value) => value,
                Err(message) => {
                    unsafe { error(buffer, ABI_INVALID_INPUT, message) };
                    return None;
                }
            };
            let fault = match unsafe { Owned::new(function.fault_type) } {
                Ok(value) => value,
                Err(message) => {
                    unsafe { error(buffer, ABI_INVALID_INPUT, message) };
                    return None;
                }
            };
            unsafe { input_owner.take_from(input) };
            Some(PreparedSync {
                input: input_owner,
                output,
                fault,
                invoke,
                unit_output: output_type.size == 0,
            })
        }

        unsafe fn blocking_operation(
            function: *const SyncFunction,
            input: *mut c_void,
            buffer: *mut Buffer,
        ) -> *mut AsyncOp {
            let Some(prepared) = (unsafe { prepare_sync(function, input, buffer) }) else {
                return ptr::null_mut();
            };
            operation(async move {
                task::blocking_typed(move || prepared.execute())
                    .await
                    .map(OpValue::Value)
            })
        }

        /// # Safety
        /// input is one initialized owned value; after a non-null return the
        /// caller must disarm it. The checked callback cannot suspend.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_blocking(
            function: *const SyncFunction,
            input: *mut c_void,
            buffer: *mut Buffer,
        ) -> *mut AsyncOp {
            unsafe { blocking_operation(function, input, buffer) }
        }

        /// # Safety
        /// Same contract as async_blocking; both use the bounded blocking pool.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_parallel(
            function: *const SyncFunction,
            input: *mut c_void,
            buffer: *mut Buffer,
        ) -> *mut AsyncOp {
            unsafe { blocking_operation(function, input, buffer) }
        }

        /// # Safety
        /// state and operation are live exclusive owners; the operation is
        /// polled by only one coroutine and the error slot is writable.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_op_poll(
            state: *mut PollState,
            operation: *mut AsyncOp,
            output: *mut c_void,
            present: *mut u8,
            fault: *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if !unsafe { empty(buffer) } {
                return 4;
            }
            let (Some(state), Some(operation)) =
                (unsafe { state.as_mut() }, unsafe { operation.as_mut() })
            else {
                return unsafe { error(buffer, 4, "missing async operation") };
            };
            if operation.completed || state.current.is_null() {
                return unsafe { error(buffer, 4, "async operation is not polling") };
            }
            // Reject invalid protocol destinations before advancing network I/O,
            // consuming a frame, or committing a response. The operation remains
            // unpolled, so a caller may retry with valid output slots.
            if let Some(slots) = operation.output_slots
                && let Err(message) = slots.validate(output, present, fault)
            {
                return unsafe { error(buffer, 4, message) };
            }
            // SAFETY: current points to the Context for this one active poll.
            let context = unsafe { &mut *(state.current as *mut Context<'_>) };
            match operation.future.as_mut().poll(context) {
                Poll::Pending => 0,
                Poll::Ready(Ok(value)) => {
                    operation.completed = true;
                    match value {
                        #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
                        OpValue::Database(value) => unsafe { value.write(output, present) },
                        OpValue::Unit => {}
                        OpValue::Value(mut value) => {
                            if output.is_null() {
                                return unsafe { error(buffer, 4, "missing async output") };
                            }
                            // SAFETY: output is the descriptor's writable slot.
                            unsafe { value.move_to(output) };
                        }
                        OpValue::OptionalValue(value) => {
                            if present.is_null() || !present.is_aligned() {
                                return unsafe {
                                    error(buffer, 4, "missing async presence output")
                                };
                            }
                            match value {
                                Some(mut value) => {
                                    if output.is_null() {
                                        return unsafe { error(buffer, 4, "missing async output") };
                                    }
                                    // SAFETY: presence and value slots are distinct.
                                    unsafe {
                                        value.move_to(output);
                                        present.write(1)
                                    };
                                }
                                None => unsafe { present.write(0) },
                            }
                        }
                        OpValue::Task(task) => {
                            if output.is_null() || !output.is_aligned() {
                                return unsafe { error(buffer, 4, "missing task output") };
                            }
                            // SAFETY: output is one writable pointer slot.
                            let handle = TaskHandle(Arc::new(Mutex::new(Some(task))));
                            unsafe {
                                (output as *mut *mut c_void)
                                    .write(Box::into_raw(Box::new(handle)).cast())
                            };
                        }
                        OpValue::StreamRow(value) => {
                            if present.is_null() || output.is_null() {
                                return unsafe { error(buffer, 4, "missing stream output") };
                            }
                            unsafe { present.write(u8::from(value.is_some())) };
                            if let Some(value) = value {
                                match value {
                                    StreamRow::Managed(value) => {
                                        if let Err(failure) = unsafe { value.copy_to(output) } {
                                            return unsafe { error(buffer, 4, failure.message) };
                                        }
                                    }
                                    StreamRow::Moved(mut value) => unsafe { value.move_to(output) },
                                    StreamRow::Int(value) => unsafe {
                                        output.cast::<i64>().write(value)
                                    },
                                }
                            }
                        }
                        OpValue::Socket(value) => {
                            if output.is_null() {
                                return unsafe { error(buffer, 4, "missing socket output") };
                            }
                            unsafe { output.cast::<*mut c_void>().write(managed::owned(value)) };
                        }
                        OpValue::Listener(value) => {
                            if output.is_null() {
                                return unsafe { error(buffer, 4, "missing listener output") };
                            }
                            unsafe { output.cast::<*mut c_void>().write(managed::owned(value)) };
                        }
                        OpValue::Bytes(value) => {
                            if present.is_null() || output.is_null() {
                                return unsafe { error(buffer, 4, "missing read output") };
                            }
                            unsafe { present.write(u8::from(value.is_some())) };
                            if let Some(value) = value {
                                unsafe {
                                    output.cast::<*mut c_void>().write(managed::owned(value))
                                };
                            }
                        }
                        OpValue::Bool(value) => {
                            if output.is_null() {
                                return unsafe { error(buffer, 4, "missing bool output") };
                            }
                            unsafe { output.cast::<u8>().write(u8::from(value)) };
                        }
                        OpValue::OptionalUnit(value) => {
                            if present.is_null() {
                                return unsafe { error(buffer, 4, "missing timeout presence") };
                            }
                            unsafe { present.write(u8::from(value)) };
                        }
                        OpValue::Protocol(value) => {
                            if let Err(message) = unsafe { value.write(output, present) } {
                                return unsafe { error(buffer, 4, message) };
                            }
                        }
                        #[cfg(feature = "runtime-api")]
                        OpValue::Api(value) => unsafe { value.write(output) },
                        #[cfg(feature = "runtime-external")]
                        OpValue::External(value) => unsafe {
                            output.cast::<*mut c_void>().write(managed::owned(value))
                        },
                    }
                    1
                }
                Poll::Ready(Err(ForeignFault::Typed(mut value))) => {
                    operation.completed = true;
                    if fault.is_null() {
                        return unsafe { error(buffer, 4, "missing async fault output") };
                    }
                    // SAFETY: fault is one writable slot of the fault type.
                    unsafe { value.move_to(fault) };
                    2
                }
                Poll::Ready(Err(ForeignFault::Runtime(message))) => {
                    operation.completed = true;
                    unsafe { error(buffer, 3, message) }
                }
            }
        }

        /// # Safety
        /// operation is one still-owned handle returned by async_sleep.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_op_release(operation: *mut AsyncOp) {
            if !operation.is_null() {
                drop(unsafe { Box::from_raw(operation) });
            }
        }

        /// # Safety
        /// The process-static descriptor and borrowed input satisfy create's
        /// contract. A child frame, including all suspend-live locals, must
        /// have passed the compiler's Send check.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_task_run(
            function: *const AsyncFunction,
            input: *const c_void,
            buffer: *mut Buffer,
        ) -> *mut AsyncOp {
            if !unsafe { empty(buffer) } {
                return ptr::null_mut();
            }
            let Some(function) = (unsafe { function.as_ref() }) else {
                unsafe {
                    error(
                        buffer,
                        ABI_INVALID_INPUT,
                        "missing async function descriptor",
                    )
                };
                return ptr::null_mut();
            };
            let (Some(output), Some(fault)) = (unsafe { function.output_type.as_ref() }, unsafe {
                function.fault_type.as_ref()
            }) else {
                unsafe { error(buffer, ABI_INVALID_INPUT, "missing async result descriptor") };
                return ptr::null_mut();
            };
            if function.send_safe == 0 || output.transferable == 0 || fault.transferable == 0 {
                unsafe {
                    error(
                        buffer,
                        ABI_INVALID_INPUT,
                        "async task frame is not transferable",
                    )
                };
                return ptr::null_mut();
            }
            let future = match unsafe { ForeignFuture::new(function, input) } {
                Ok(future) => SendForeignFuture(future),
                Err(message) => {
                    unsafe { error(buffer, ABI_INVALID_INPUT, message) };
                    return ptr::null_mut();
                }
            };
            if output.size == 0 {
                operation(async move {
                    task::run_typed(async move { future.await.map(drop) })
                        .await
                        .map(TaskKind::Unit)
                        .map(OpValue::Task)
                        .map_err(ForeignFault::from)
                })
            } else {
                operation(async move {
                    task::run_typed(future)
                        .await
                        .map(|task| TaskKind::Value(task, output))
                        .map(OpValue::Task)
                        .map_err(ForeignFault::from)
                })
            }
        }

        /// # Safety
        /// input is one initialized owned row, disarmed after a non-null
        /// return. The synchronous callback runs in the bounded blocking pool.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_task_run_sync(
            function: *const SyncFunction,
            input: *mut c_void,
            buffer: *mut Buffer,
        ) -> *mut AsyncOp {
            let Some(prepared) = (unsafe { prepare_sync(function, input, buffer) }) else {
                return ptr::null_mut();
            };
            let output_type = prepared.output.ty;
            if prepared.unit_output {
                operation(async move {
                    task::run_typed(async move {
                        task::blocking_typed(move || prepared.execute())
                            .await
                            .map(drop)
                    })
                    .await
                    .map(TaskKind::Unit)
                    .map(OpValue::Task)
                    .map_err(ForeignFault::from)
                })
            } else {
                operation(async move {
                    task::run_typed(async move {
                        task::blocking_typed(move || prepared.execute()).await
                    })
                    .await
                    .map(|task| TaskKind::Value(task, output_type))
                    .map(OpValue::Task)
                    .map_err(ForeignFault::from)
                })
            }
        }

        /// # Safety
        /// task is one exclusive owned handle and is consumed by this call.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_task_wait(task: *mut c_void) -> *mut AsyncOp {
            if task.is_null() {
                return ptr::null_mut();
            }
            let task = take_task(*unsafe { Box::from_raw(task.cast::<TaskHandle>()) });
            operation(async move {
                match task? {
                    TaskKind::Value(task, _) => task::wait(task).await.map(OpValue::Value),
                    TaskKind::Unit(task) => task::wait(task).await.map(|()| OpValue::Unit),
                }
            })
        }

        /// # Safety
        /// task is one exclusive unit-task handle consumed by this call.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_task_stop(task: *mut c_void) -> *mut AsyncOp {
            if task.is_null() {
                return ptr::null_mut();
            }
            let task = take_task(*unsafe { Box::from_raw(task.cast::<TaskHandle>()) });
            operation(async move {
                match task? {
                    TaskKind::Unit(task) => task::stop(task).await.map(|()| OpValue::Unit),
                    TaskKind::Value(_, _) => Err(ForeignFault::from(
                        "only a unit task can be stopped".to_owned(),
                    )),
                }
            })
        }

        /// # Safety
        /// task transfers one live wrapper. The returned operation owns it.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_task_wait_timeout(
            task: *mut c_void,
            milliseconds: i64,
        ) -> *mut AsyncOp {
            if task.is_null() {
                return ptr::null_mut();
            }
            let task = take_task(*unsafe { Box::from_raw(task.cast::<TaskHandle>()) });
            operation(async move {
                match task? {
                    TaskKind::Value(task, _) => task::wait_timeout(task, milliseconds)
                        .await
                        .map(OpValue::OptionalValue),
                    TaskKind::Unit(task) => task::wait_timeout(task, milliseconds)
                        .await
                        .map(|value| OpValue::OptionalUnit(value.is_some())),
                }
            })
        }

        /// # Safety
        /// tasks is a borrowed array of distinct live wrappers of one output type.
        /// Non-null return consumes every wrapper; null return consumes none.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_task_race(
            tasks: *const *mut c_void,
            count: u64,
            buffer: *mut Buffer,
        ) -> *mut AsyncOp {
            if !unsafe { empty(buffer) } {
                return ptr::null_mut();
            }
            let prepared = (|| {
                let count = usize::try_from(count).map_err(|_| "invalid race task count")?;
                if count < 2
                    || count > isize::MAX as usize / size_of::<*mut c_void>()
                    || tasks.is_null()
                    || !tasks.is_aligned()
                {
                    return Err("race requires an array of at least two tasks");
                }
                let pointers = unsafe { std::slice::from_raw_parts(tasks, count) };
                let mut handles = Vec::with_capacity(count);
                let mut identities = std::collections::HashSet::with_capacity(count);
                for pointer in pointers {
                    let handle = unsafe { pointer.cast::<TaskHandle>().as_ref() }
                        .ok_or("missing race task")?;
                    if !identities.insert(Arc::as_ptr(&handle.0)) {
                        return Err("race tasks must be distinct");
                    }
                    handles.push(handle);
                }
                // Hold all locks until validation completes: errors never partly consume tasks.
                let mut slots = handles
                    .iter()
                    .map(|handle| handle.0.lock().map_err(|_| "async task state lock failed"))
                    .collect::<Result<Vec<_>, _>>()?;
                let output_type = slots[0]
                    .as_ref()
                    .ok_or("async task was already consumed")?
                    .output_type()
                    .map(std::ptr::from_ref);
                let unit = output_type.is_none();
                for slot in &slots {
                    let task = slot.as_ref().ok_or("async task was already consumed")?;
                    if task.output_type().map(std::ptr::from_ref) != output_type {
                        return Err("race task outputs must agree");
                    }
                }
                let tasks = slots
                    .iter_mut()
                    .map(|slot| slot.take().expect("validated race task"))
                    .collect::<Vec<_>>();
                drop(slots);
                for pointer in pointers {
                    drop(unsafe { Box::from_raw(pointer.cast::<TaskHandle>()) });
                }
                Ok((unit, tasks))
            })();
            let (unit, tasks) = match prepared {
                Ok(value) => value,
                Err(message) => {
                    unsafe { error(buffer, ABI_INVALID_INPUT, message) };
                    return ptr::null_mut();
                }
            };
            operation(async move {
                if unit {
                    let tasks = tasks
                        .into_iter()
                        .map(|task| match task {
                            TaskKind::Unit(task) => task,
                            TaskKind::Value(_, _) => unreachable!("validated race outputs"),
                        })
                        .collect();
                    task::race(tasks).await.map(|()| OpValue::Unit)
                } else {
                    let tasks = tasks
                        .into_iter()
                        .map(|task| match task {
                            TaskKind::Value(task, _) => task,
                            TaskKind::Unit(_) => unreachable!("validated race outputs"),
                        })
                        .collect();
                    task::race(tasks).await.map(OpValue::Value)
                }
            })
        }

        /// # Safety
        /// task is a live borrowed wrapper. The inner task remains affine.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_task_retain(task: *const c_void) -> *mut c_void {
            unsafe { task.cast::<TaskHandle>().as_ref() }
                .map(|task| Box::into_raw(Box::new(TaskHandle(Arc::clone(&task.0)))).cast())
                .unwrap_or(ptr::null_mut())
        }

        /// # Safety
        /// task is one exclusive owned handle, released at most once.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_task_release(task: *mut c_void) {
            if !task.is_null() {
                drop(unsafe { Box::from_raw(task.cast::<TaskHandle>()) });
            }
        }

        /// # Safety
        /// output is one writable pointer slot; error is a writable buffer.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_group_new(
            limit: i64,
            output: *mut *mut GroupHandle,
            buffer: *mut Buffer,
        ) -> u32 {
            if !unsafe { empty(buffer) } || output.is_null() || !output.is_aligned() {
                return ABI_INVALID_INPUT;
            }
            match task::Group::<ForeignFault>::new_typed(limit) {
                Ok(group) => {
                    unsafe {
                        output.write(Box::into_raw(Box::new(GroupHandle(Arc::new(Mutex::new(
                            Some(group),
                        ))))))
                    };
                    0
                }
                Err(message) => unsafe { error(buffer, 3, message) },
            }
        }

        /// # Safety
        /// group is live and function is a process-static checked unit future.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_group_run(
            group: *mut GroupHandle,
            function: *const AsyncFunction,
            input: *const c_void,
            buffer: *mut Buffer,
        ) -> *mut AsyncOp {
            if !unsafe { empty(buffer) } {
                return ptr::null_mut();
            }
            let (Some(group), Some(function)) =
                (unsafe { group.as_ref() }, unsafe { function.as_ref() })
            else {
                unsafe { error(buffer, ABI_INVALID_INPUT, "missing async group or function") };
                return ptr::null_mut();
            };
            let (Some(output), Some(fault)) = (unsafe { function.output_type.as_ref() }, unsafe {
                function.fault_type.as_ref()
            }) else {
                unsafe { error(buffer, ABI_INVALID_INPUT, "missing async result descriptor") };
                return ptr::null_mut();
            };
            if output.size != 0 || function.send_safe == 0 || fault.transferable == 0 {
                unsafe {
                    error(
                        buffer,
                        ABI_INVALID_INPUT,
                        "group child must be a transferable unit future",
                    )
                };
                return ptr::null_mut();
            }
            let future = match unsafe { ForeignFuture::new(function, input) } {
                Ok(future) => SendForeignFuture(future),
                Err(message) => {
                    unsafe { error(buffer, ABI_INVALID_INPUT, message) };
                    return ptr::null_mut();
                }
            };
            let shared = Arc::clone(&group.0);
            operation(async move {
                let mut lease = GroupLease::take(shared)?;
                lease
                    .group
                    .as_mut()
                    .expect("leased group")
                    .run(async move { future.await.map(drop) })
                    .await?;
                Ok(OpValue::Unit)
            })
        }

        /// # Safety
        /// group is a live borrowed wrapper; input is one initialized owned
        /// row, disarmed after a non-null return. Output must be Unit.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_group_run_sync(
            group: *mut GroupHandle,
            function: *const SyncFunction,
            input: *mut c_void,
            buffer: *mut Buffer,
        ) -> *mut AsyncOp {
            if !unsafe { empty(buffer) } {
                return ptr::null_mut();
            }
            let Some(group) = (unsafe { group.as_ref() }) else {
                unsafe { error(buffer, ABI_INVALID_INPUT, "missing async group") };
                return ptr::null_mut();
            };
            if unsafe { function.as_ref() }
                .and_then(|function| unsafe { function.output_type.as_ref() })
                .is_none_or(|output| output.size != 0)
            {
                unsafe { error(buffer, ABI_INVALID_INPUT, "group child must return Unit") };
                return ptr::null_mut();
            }
            let Some(prepared) = (unsafe { prepare_sync(function, input, buffer) }) else {
                return ptr::null_mut();
            };
            let shared = Arc::clone(&group.0);
            operation(async move {
                let mut lease = GroupLease::take(shared)?;
                lease
                    .group
                    .as_mut()
                    .expect("leased group")
                    .run(async move {
                        task::blocking_typed(move || prepared.execute())
                            .await
                            .map(drop)
                    })
                    .await?;
                Ok(OpValue::Unit)
            })
        }

        /// # Safety
        /// group is consumed and no group operation may still borrow it.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_group_wait(
            group: *mut GroupHandle,
        ) -> *mut AsyncOp {
            if group.is_null() {
                return ptr::null_mut();
            }
            let group = *unsafe { Box::from_raw(group) };
            operation(async move {
                let group = GroupLease::take(group.0)?
                    .group
                    .take()
                    .expect("leased group");
                group.wait().await.map(|()| OpValue::Unit)
            })
        }

        /// # Safety
        /// group is consumed and no group operation may still borrow it.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_group_stop(
            group: *mut GroupHandle,
        ) -> *mut AsyncOp {
            if group.is_null() {
                return ptr::null_mut();
            }
            let group = *unsafe { Box::from_raw(group) };
            operation(async move {
                let group = GroupLease::take(group.0)?
                    .group
                    .take()
                    .expect("leased group");
                group.stop().await.map(|()| OpValue::Unit)
            })
        }

        /// # Safety
        /// group is a live borrowed wrapper. The inner group remains affine.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_group_retain(
            group: *const GroupHandle,
        ) -> *mut GroupHandle {
            unsafe { group.as_ref() }
                .map(|group| Box::into_raw(Box::new(GroupHandle(Arc::clone(&group.0)))))
                .unwrap_or(ptr::null_mut())
        }

        /// # Safety
        /// group is one exclusive owned handle.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_group_release(group: *mut GroupHandle) {
            if !group.is_null() {
                drop(unsafe { Box::from_raw(group) });
            }
        }

        /// # Safety
        /// output is a writable pointer slot; error is a writable buffer.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_channel_new(
            capacity: i64,
            output: *mut *mut ChannelHandle,
            buffer: *mut Buffer,
        ) -> u32 {
            if !unsafe { empty(buffer) } || output.is_null() || !output.is_aligned() {
                return ABI_INVALID_INPUT;
            }
            match Channel::new(capacity) {
                Ok(channel) => {
                    unsafe { output.write(Box::into_raw(Box::new(ChannelHandle(channel)))) };
                    0
                }
                Err(message) => unsafe { error(buffer, 3, message) },
            }
        }

        /// # Safety
        /// channel is a live borrowed handle.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_channel_retain(
            channel: *const ChannelHandle,
        ) -> *mut ChannelHandle {
            unsafe { channel.as_ref() }
                .map(|channel| Box::into_raw(Box::new(ChannelHandle(channel.0.clone()))))
                .unwrap_or(ptr::null_mut())
        }

        /// # Safety
        /// channel is one owned handle.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_channel_release(channel: *mut ChannelHandle) {
            if !channel.is_null() {
                drop(unsafe { Box::from_raw(channel) });
            }
        }

        /// # Safety
        /// element is one initialized concrete value, consumed on success;
        /// descriptor and channel match its compiler-checked element type.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_channel_send(
            channel: *mut ChannelHandle,
            element_type: *const OwnedType,
            element: *mut c_void,
            buffer: *mut Buffer,
        ) -> *mut AsyncOp {
            if !unsafe { empty(buffer) } {
                return ptr::null_mut();
            }
            let Some(channel) = (unsafe { channel.as_ref() }) else {
                unsafe { error(buffer, ABI_INVALID_INPUT, "missing async channel") };
                return ptr::null_mut();
            };
            if element.is_null()
                || unsafe { element_type.as_ref() }.is_none_or(|ty| ty.transferable == 0)
            {
                unsafe {
                    error(
                        buffer,
                        ABI_INVALID_INPUT,
                        "channel element is not transferable",
                    )
                };
                return ptr::null_mut();
            }
            let mut value = match unsafe { Owned::new(element_type) } {
                Ok(value) => value,
                Err(message) => {
                    unsafe { error(buffer, ABI_INVALID_INPUT, message) };
                    return ptr::null_mut();
                }
            };
            unsafe { value.take_from(element) };
            let channel = channel.0.clone();
            operation(async move {
                channel.send(value).await.map_err(ForeignFault::from)?;
                Ok(OpValue::Unit)
            })
        }

        /// # Safety
        /// channel is a live borrowed handle.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_channel_receive(
            channel: *mut ChannelHandle,
        ) -> *mut AsyncOp {
            let Some(channel) = (unsafe { channel.as_ref() }) else {
                return ptr::null_mut();
            };
            let channel = channel.0.clone();
            operation(async move {
                channel
                    .receive()
                    .await
                    .map(OpValue::OptionalValue)
                    .map_err(ForeignFault::from)
            })
        }

        /// # Safety
        /// channel is a live borrowed handle.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_channel_close(
            channel: *mut ChannelHandle,
        ) -> *mut AsyncOp {
            let Some(channel) = (unsafe { channel.as_ref() }) else {
                return ptr::null_mut();
            };
            let channel = channel.0.clone();
            operation(async move {
                channel
                    .close()
                    .await
                    .map(|()| OpValue::Unit)
                    .map_err(ForeignFault::from)
            })
        }

        unsafe fn stream_element_type(
            ty: *const OwnedType,
            size: usize,
            align: usize,
        ) -> Result<(), managed::AbiFailure> {
            let ty =
                unsafe { ty.as_ref() }.ok_or_else(|| managed::invalid("missing stream type"))?;
            if ty.transferable == 0
                || ty.size != size as u64
                || ty.align != align as u64
                || ty.move_init.is_none()
                || ty.drop.is_none()
            {
                return Err(managed::invalid(
                    "invalid or non-transferable stream element type",
                ));
            }
            Ok(())
        }

        unsafe fn memory_rows(
            source: *const c_void,
            kind: u32,
        ) -> Result<Box<dyn Iterator<Item = StreamRow> + Send>, String> {
            match kind {
                0 => {
                    let list = unsafe { managed::borrowed::<managed::ListHandle>(source) }
                        .map_err(|failure| failure.message)?;
                    Ok(Box::new(
                        list.values.clone().into_values().map(StreamRow::Managed),
                    ))
                }
                1 => {
                    let bytes = unsafe { managed::borrowed::<Bytes>(source) }
                        .map_err(|failure| failure.message)?;
                    Ok(Box::new(
                        bytes
                            .clone()
                            .into_values()
                            .map(|value| StreamRow::Int(i64::from(value))),
                    ))
                }
                _ => Err("parallel memory source must be List or Bytes".into()),
            }
        }

        /// # Safety
        /// source and context are borrowed matching the static descriptor; its
        /// Send contract covers the source row and the complete packed inputs.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_parallel_each(
            source: *const c_void,
            kind: u32,
            limit: i64,
            function: *const ParallelFunction,
            context: *const c_void,
            buffer: *mut Buffer,
        ) -> *mut AsyncOp {
            if !unsafe { empty(buffer) } {
                return ptr::null_mut();
            }
            let prepared = (|| {
                let handler = Arc::new(unsafe { ParallelHandler::new(function, context) }?);
                if kind == 3 {
                    let stream = unsafe { managed::borrowed::<AsyncStreamHandle>(source) }
                        .map_err(|failure| failure.message)?
                        .0
                        .clone();
                    Ok::<_, String>((handler, None, Some(stream)))
                } else {
                    let rows = unsafe { memory_rows(source, kind) }?;
                    if handler.asynchronous.is_some() {
                        Ok((handler, None, Some(AsyncStream::from_values(rows))))
                    } else {
                        Ok((handler, Some(rows), None))
                    }
                }
            })();
            let (handler, rows, stream) = match prepared {
                Ok(prepared) => prepared,
                Err(message) => {
                    unsafe { error(buffer, ABI_INVALID_INPUT, message) };
                    return ptr::null_mut();
                }
            };
            operation(async move {
                if let Some(rows) = rows {
                    task::parallel_each_typed(rows, limit, move |row| handler.execute(row)).await?;
                } else {
                    let stream = stream.expect("validated parallel stream");
                    if handler.asynchronous.is_some() {
                        task::parallel_each_stream_typed(stream, limit, move |row| {
                            let future = handler.start(row);
                            async move { future?.await.map(drop) }
                        })
                        .await?;
                    } else {
                        task::parallel_each_stream_typed(stream, limit, move |row| {
                            let handler = Arc::clone(&handler);
                            async move { handler.execute(row) }
                        })
                        .await?;
                    }
                }
                Ok(OpValue::Unit)
            })
        }

        /// # Safety
        /// source/context match the checked handler; fault/error are distinct
        /// writable slots. The synchronous Stream producer stays on this thread.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_parallel_each(
            source: *const c_void,
            kind: u32,
            limit: i64,
            function: *const ParallelFunction,
            context: *const c_void,
            fault: *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if !unsafe { empty(buffer) } || fault.is_null() {
                return 4;
            }
            let handler = match unsafe { ParallelHandler::new(function, context) } {
                Ok(handler) if handler.synchronous.is_some() => handler,
                Ok(_) => {
                    return unsafe {
                        error(
                            buffer,
                            4,
                            "synchronous parallel_each requires a synchronous handler",
                        )
                    };
                }
                Err(message) => return unsafe { error(buffer, 4, message) },
            };
            let result = match kind {
                0 => match unsafe { managed::borrowed::<managed::ListHandle>(source) } {
                    Ok(list) => dever_runtime::concurrent::each_slice_typed(
                        list.values.values(),
                        limit,
                        |row| handler.execute(StreamRow::Managed(row.clone())),
                    ),
                    Err(failure) => return unsafe { error(buffer, 4, failure.message) },
                },
                1 => match unsafe { managed::borrowed::<Bytes>(source) } {
                    Ok(bytes) => {
                        dever_runtime::concurrent::each_slice_typed(bytes.values(), limit, |row| {
                            handler.execute(StreamRow::Int(i64::from(*row)))
                        })
                    }
                    Err(failure) => return unsafe { error(buffer, 4, failure.message) },
                },
                2 => match unsafe { managed::borrowed::<managed::StreamHandle>(source) } {
                    Ok(stream) => dever_runtime::concurrent::each_typed(
                        std::iter::from_fn(|| stream.values.pull()).map(StreamRow::Managed),
                        limit,
                        |row| handler.execute(row),
                    ),
                    Err(failure) => return unsafe { error(buffer, 4, failure.message) },
                },
                _ => return unsafe { error(buffer, 4, "invalid synchronous parallel source") },
            };
            match result {
                Ok(()) => 1,
                Err(ForeignFault::Typed(mut value)) => {
                    unsafe { value.move_to(fault) };
                    2
                }
                Err(ForeignFault::Runtime(message)) => unsafe { error(buffer, 3, message) },
            }
        }

        /// # Safety
        /// list is borrowed; descriptor names its exact transferable element type.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_stream_from_list(
            list: *const c_void,
            ty: *const OwnedType,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                return status;
            }
            let result = (|| {
                let list = unsafe { managed::borrowed::<managed::ListHandle>(list) }?;
                unsafe { stream_element_type(ty, list.element.size, list.element.align) }?;
                Ok(AsyncStreamHandle(AsyncStream::from_values(
                    list.values.clone().into_values().map(StreamRow::Managed),
                )))
            })();
            unsafe { managed::complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// bytes is borrowed; ty is the process-static Int descriptor.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_stream_from_bytes(
            bytes: *const c_void,
            ty: *const OwnedType,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                return status;
            }
            let result = (|| {
                unsafe { stream_element_type(ty, size_of::<i64>(), align_of::<i64>()) }?;
                let bytes = unsafe { managed::borrowed::<Bytes>(bytes) }?.clone();
                Ok(AsyncStreamHandle(AsyncStream::from_values(
                    bytes
                        .into_values()
                        .map(|value| StreamRow::Int(i64::from(value))),
                )))
            })();
            unsafe { managed::complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// ty names Int; output/error are distinct writable slots in an active Scope.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_ticks(
            milliseconds: i64,
            ty: *const OwnedType,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                return status;
            }
            let result = unsafe { stream_element_type(ty, size_of::<i64>(), align_of::<i64>()) }
                .and_then(|()| {
                    task::ticks(milliseconds)
                        .map(|stream| AsyncStreamHandle(stream.map(StreamRow::Int)))
                        .map_err(managed::runtime_error)
                });
            unsafe { managed::complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// channel is borrowed and contains checked transferable typed values.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_channel_stream(
            channel: *const ChannelHandle,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                return status;
            }
            let result = unsafe { channel.as_ref() }
                .ok_or_else(|| managed::invalid("missing async channel"))
                .map(|channel| AsyncStreamHandle(channel.0.clone().stream().map(StreamRow::Moved)));
            unsafe { managed::complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// stream is a live borrowed AsyncStream handle, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_stream_retain(
            stream: *const c_void,
        ) -> *mut c_void {
            unsafe { managed::retain::<AsyncStreamHandle>(stream) }
        }

        /// # Safety
        /// stream transfers one AsyncStream handle, or is null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_stream_release(stream: *mut c_void) {
            unsafe { managed::release::<AsyncStreamHandle>(stream) }
        }

        /// # Safety
        /// stream is a borrowed AsyncStream handle; operation retains its cursor.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_stream_pull(
            stream: *const c_void,
        ) -> *mut AsyncOp {
            let Ok(stream) = (unsafe { managed::borrowed::<AsyncStreamHandle>(stream) }) else {
                return ptr::null_mut();
            };
            let stream = stream.0.clone();
            operation(async move { Ok(OpValue::StreamRow(stream.pull().await)) })
        }

        /// # Safety
        /// stream is borrowed; output/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_stream_close(
            stream: *const c_void,
            out: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                return status;
            }
            let result = unsafe { managed::borrowed::<AsyncStreamHandle>(stream) }.map(|stream| {
                stream.0.close();
                1
            });
            unsafe { managed::complete(out, buffer, result) }
        }

        /// # Safety
        /// host is a borrowed Text handle, cloned before the operation suspends.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_net_connect(
            host: *const c_void,
            port: i64,
        ) -> *mut AsyncOp {
            let Ok(host) = (unsafe { managed::borrowed::<String>(host) }) else {
                return ptr::null_mut();
            };
            let host = host.clone();
            operation(async move {
                net::connect(&host, port)
                    .await
                    .map(OpValue::Socket)
                    .map_err(ForeignFault::from)
            })
        }

        /// # Safety
        /// host is a borrowed Text handle, cloned before the operation suspends.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_net_connect_timeout(
            host: *const c_void,
            port: i64,
            milliseconds: i64,
        ) -> *mut AsyncOp {
            let Ok(host) = (unsafe { managed::borrowed::<String>(host) }) else {
                return ptr::null_mut();
            };
            let host = host.clone();
            operation(async move {
                net::connect_timeout(&host, port, milliseconds)
                    .await
                    .map(OpValue::Socket)
                    .map_err(ForeignFault::from)
            })
        }

        /// # Safety
        /// host is a borrowed Text handle, cloned before the operation suspends.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_net_listen(
            host: *const c_void,
            port: i64,
        ) -> *mut AsyncOp {
            let Ok(host) = (unsafe { managed::borrowed::<String>(host) }) else {
                return ptr::null_mut();
            };
            let host = host.clone();
            operation(async move {
                net::listen(&host, port)
                    .await
                    .map(OpValue::Listener)
                    .map_err(ForeignFault::from)
            })
        }

        /// # Safety
        /// listener is a borrowed Listener handle.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_net_accept(
            listener: *const c_void,
        ) -> *mut AsyncOp {
            let Ok(listener) = (unsafe { managed::borrowed::<net::Listener>(listener) }) else {
                return ptr::null_mut();
            };
            let listener = listener.clone();
            operation(async move {
                net::accept(&listener)
                    .await
                    .map(OpValue::Socket)
                    .map_err(ForeignFault::from)
            })
        }

        /// # Safety
        /// socket is a borrowed Socket handle.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_net_read(
            socket: *const c_void,
            limit: i64,
        ) -> *mut AsyncOp {
            let Ok(socket) = (unsafe { managed::borrowed::<net::Socket>(socket) }) else {
                return ptr::null_mut();
            };
            let socket = socket.clone();
            operation(async move {
                net::read(&socket, limit)
                    .await
                    .map(OpValue::Bytes)
                    .map_err(ForeignFault::from)
            })
        }

        /// # Safety
        /// socket and bytes are borrowed handles, cloned before suspension.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_net_write(
            socket: *const c_void,
            bytes: *const c_void,
        ) -> *mut AsyncOp {
            let (Ok(socket), Ok(bytes)) = (
                unsafe { managed::borrowed::<net::Socket>(socket) },
                unsafe { managed::borrowed::<Bytes>(bytes) },
            ) else {
                return ptr::null_mut();
            };
            let (socket, bytes) = (socket.clone(), bytes.clone());
            operation(async move {
                net::write(&socket, &bytes)
                    .await
                    .map(|()| OpValue::Bool(true))
                    .map_err(ForeignFault::from)
            })
        }

        struct StreamEvent {
            element: managed::ValueOps,
            success: unsafe extern "C" fn(*const c_void, *mut c_void),
            failed: unsafe extern "C" fn(*const c_void, *mut c_void),
        }

        impl StreamEvent {
            unsafe fn new(
                event: *const managed::ReadEventDescriptor,
                ty: *const OwnedType,
            ) -> Result<Self, managed::AbiFailure> {
                let event = unsafe { event.as_ref() }
                    .ok_or_else(|| managed::invalid("missing stream event descriptor"))?;
                let element = unsafe { managed::descriptor(event.element_type, false) }?;
                unsafe { stream_element_type(ty, element.size, element.align) }?;
                let success = event
                    .chunk
                    .ok_or_else(|| managed::invalid("missing stream success callback"))?;
                let failed = event
                    .failed
                    .ok_or_else(|| managed::invalid("missing stream failure callback"))?;
                Ok(Self {
                    element,
                    success,
                    failed,
                })
            }

            fn row<T>(&self, result: Result<T, String>) -> StreamRow {
                StreamRow::Managed(match result {
                    Ok(value) => managed::initialize_event(value, self.element, self.success),
                    Err(message) => managed::initialize_event(message, self.element, self.failed),
                })
            }
        }

        /// # Safety
        /// socket is borrowed; callbacks initialize the exact transferable event type.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_net_chunks(
            socket: *const c_void,
            limit: i64,
            event: *const managed::ReadEventDescriptor,
            ty: *const OwnedType,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                return status;
            }
            let result = (|| {
                let event = unsafe { StreamEvent::new(event, ty) }?;
                let socket = unsafe { managed::borrowed::<net::Socket>(socket) }?.clone();
                let stream = net::chunks(socket, limit).map_err(managed::runtime_error)?;
                Ok(AsyncStreamHandle(stream.map(move |value| event.row(value))))
            })();
            unsafe { managed::complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// listener is borrowed; callbacks initialize the exact transferable event type.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_net_connections(
            listener: *const c_void,
            event: *const managed::ReadEventDescriptor,
            ty: *const OwnedType,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                return status;
            }
            let result = (|| {
                let event = unsafe { StreamEvent::new(event, ty) }?;
                let listener = unsafe { managed::borrowed::<net::Listener>(listener) }?.clone();
                Ok(AsyncStreamHandle(
                    net::connections(listener).map(move |value| event.row(value)),
                ))
            })();
            unsafe { managed::complete_handle(out, buffer, result) }
        }

        #[cfg(feature = "runtime-api")]
        mod api_application {
            use super::*;
            use dever_runtime::{api, application, auth, config, http};

            pub(super) enum Output {
                UploadId(dever_runtime::orm::Uuid),
                #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
                Permissions(managed::ListHandle),
            }
            impl Output {
                pub(super) unsafe fn write(self, output: *mut c_void) {
                    let handle = match self {
                        Self::UploadId(value) => managed::owned(value),
                        #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
                        Self::Permissions(value) => managed::owned(value),
                    };
                    unsafe { output.cast::<*mut c_void>().write(handle) };
                }
            }

            #[repr(C)]
            #[derive(Clone, Copy)]
            pub struct Name {
                pointer: *const u8,
                len: u64,
            }
            #[repr(C)]
            pub struct Binding {
                explicit: *const Name,
                root: Name,
                tenant: u8,
            }
            #[repr(C)]
            pub struct TransactionBindings {
                bindings: *const Binding,
                count: u64,
            }
            type Decode = unsafe extern "C" fn(*mut c_void, i64, *mut c_void, *mut Buffer) -> u32;
            type FaultStatus = unsafe extern "C" fn(*const c_void) -> i64;
            type FaultMessage = unsafe extern "C" fn(*const c_void) -> *const c_void;
            type PackClaims = unsafe extern "C" fn(
                *const c_void,
                *const c_void,
                *const c_void,
                *const c_void,
                *mut c_void,
                *mut Buffer,
            ) -> u32;
            type UnpackIdentity = unsafe extern "C" fn(
                *const c_void,
                *mut *mut c_void,
                *mut i64,
                *mut u8,
                *mut i64,
                *mut u8,
                *mut Buffer,
            ) -> u32;
            #[repr(C)]
            pub struct ApiHandler {
                asynchronous: *const AsyncFunction,
                synchronous: *const SyncFunction,
                input_type: *const OwnedType,
                decode: Option<Decode>,
                fault_status: Option<FaultStatus>,
                fault_message: Option<FaultMessage>,
            }
            #[repr(C)]
            pub struct ApiAuth {
                asynchronous: *const AsyncFunction,
                synchronous: *const SyncFunction,
                input_type: *const OwnedType,
                pack: Option<PackClaims>,
                unpack: Option<UnpackIdentity>,
                fault_status: Option<FaultStatus>,
            }
            #[repr(C)]
            pub struct Permission {
                key: Name,
                component: Name,
                domain: Name,
                site: Name,
                action: Name,
                method: Name,
            }
            #[repr(C)]
            pub struct Route {
                method: Name,
                path: Name,
                directory: *const Name,
                directory_count: u64,
                components: *const Name,
                component_count: u64,
                permission: *const Permission,
                auth: *const ApiAuth,
                handler: *const ApiHandler,
                anonymous: u8,
                multipart: u8,
                detail: u8,
            }

            unsafe fn record<'a, T>(raw: *const T) -> Result<&'a T, managed::AbiFailure> {
                unsafe { managed::borrowed(raw.cast()) }
            }
            unsafe fn array<'a, T>(
                raw: *const T,
                count: u64,
            ) -> Result<&'a [T], managed::AbiFailure> {
                let count = usize::try_from(count)
                    .map_err(|_| managed::invalid("invalid API array count"))?;
                if count == 0 {
                    return Ok(&[]);
                }
                if raw.is_null()
                    || !raw.is_aligned()
                    || count > isize::MAX as usize / size_of::<T>().max(1)
                {
                    return Err(managed::invalid("invalid API array"));
                }
                Ok(unsafe { std::slice::from_raw_parts(raw, count) })
            }
            unsafe fn name<'a>(raw: &Name) -> Result<&'a str, managed::AbiFailure> {
                std::str::from_utf8(unsafe { array(raw.pointer, raw.len) }?)
                    .map_err(|_| managed::invalid("API name is not UTF-8"))
            }
            unsafe fn names(
                raw: *const Name,
                count: u64,
            ) -> Result<Vec<&'static str>, managed::AbiFailure> {
                unsafe { array(raw, count) }?
                    .iter()
                    .map(|value| unsafe { name(value) })
                    .collect()
            }
            fn flag(value: u8) -> Result<bool, managed::AbiFailure> {
                match value {
                    0 => Ok(false),
                    1 => Ok(true),
                    _ => Err(managed::invalid("invalid API boolean")),
                }
            }
            unsafe fn session(
                raw: *const c_void,
            ) -> Result<Arc<application::Session>, managed::AbiFailure> {
                unsafe { managed::borrowed::<Arc<application::Session>>(raw) }.cloned()
            }
            fn buffer() -> Buffer {
                Buffer {
                    ptr: ptr::null_mut(),
                    len: 0,
                }
            }
            fn callback(status: u32, buffer: Buffer) -> Result<(), String> {
                if status == 0 && buffer.ptr.is_null() && buffer.len == 0 {
                    return Ok(());
                }
                let message = if buffer.ptr.is_null() {
                    "API callback failed without a diagnostic".to_owned()
                } else {
                    let bytes =
                        unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len as usize) };
                    let message = String::from_utf8_lossy(bytes).into_owned();
                    unsafe { super::super::dever_rt_v1_buffer_free(buffer.ptr, buffer.len) };
                    message
                };
                Err(message)
            }

            struct Callable {
                asynchronous: Option<&'static AsyncFunction>,
                synchronous: Option<&'static SyncFunction>,
                input: &'static OwnedType,
                output: &'static OwnedType,
                fault_status: FaultStatus,
            }
            // Descriptors are immutable compiler output. Each call creates its own
            // concrete input/frame/result; source transfer rules are checked before emission.
            unsafe impl Send for Callable {}
            unsafe impl Sync for Callable {}
            impl Callable {
                unsafe fn new(
                    asynchronous: *const AsyncFunction,
                    synchronous: *const SyncFunction,
                    input: *const OwnedType,
                    fault_status: Option<FaultStatus>,
                ) -> Result<Self, managed::AbiFailure> {
                    let asynchronous = if asynchronous.is_null() {
                        None
                    } else {
                        Some(unsafe { record(asynchronous) }?)
                    };
                    let synchronous = if synchronous.is_null() {
                        None
                    } else {
                        Some(unsafe { record(synchronous) }?)
                    };
                    let (output, fault, send) = match (asynchronous, synchronous) {
                        (Some(function), None)
                            if function.create.is_some()
                                && function.resume.is_some()
                                && function.destroy.is_some()
                                && function.done.is_some() =>
                        {
                            (
                                function.output_type,
                                function.fault_type,
                                function.send_safe,
                            )
                        }
                        (None, Some(function))
                            if function.invoke.is_some() && function.input_type == input =>
                        {
                            (
                                function.output_type,
                                function.fault_type,
                                function.send_safe,
                            )
                        }
                        _ => return Err(managed::invalid("invalid API callable")),
                    };
                    for ty in [input, output, fault] {
                        let (ty, _) = unsafe { owned_layout(ty) }.map_err(managed::invalid)?;
                        if send == 0 || ty.transferable == 0 {
                            return Err(managed::invalid("API callable is not transferable"));
                        }
                    }
                    Ok(Self {
                        asynchronous,
                        synchronous,
                        input: unsafe { &*input },
                        output: unsafe { &*output },
                        fault_status: fault_status
                            .ok_or_else(|| managed::invalid("missing API failure mapping"))?,
                    })
                }
                async fn invoke(&self, input: Owned) -> Result<Owned, ForeignFault> {
                    if let Some(function) = self.asynchronous {
                        let future = unsafe { ForeignFuture::new(function, input.pointer()) }
                            .map(SendForeignFuture)
                            .map_err(ForeignFault::from)?;
                        drop(input);
                        return future.await;
                    }
                    let function = self.synchronous.expect("validated API function");
                    PreparedSync {
                        input,
                        output: unsafe { Owned::new(function.output_type) }
                            .map_err(ForeignFault::from)?,
                        fault: unsafe { Owned::new(function.fault_type) }
                            .map_err(ForeignFault::from)?,
                        invoke: function.invoke.expect("validated API function"),
                        unit_output: false,
                    }
                    .execute()
                }
                fn failure(
                    &self,
                    fault: ForeignFault,
                    message: Option<FaultMessage>,
                ) -> http::Response {
                    let status = match &fault {
                        ForeignFault::Typed(fault) => unsafe {
                            (self.fault_status)(fault.pointer())
                        },
                        ForeignFault::Runtime(_) => 500,
                    };
                    if status == 400
                        && let (ForeignFault::Typed(fault), Some(message)) = (&fault, message)
                    {
                        let text = unsafe { message(fault.pointer()) };
                        if !text.is_null() {
                            return match unsafe { managed::borrowed::<String>(text) } {
                                Ok(text) => api::invalid_input(api::InputError(text.clone())),
                                Err(_) => api::internal_error(),
                            };
                        }
                    }
                    match status {
                        400 | 401 | 403 | 404 | 409 | 429 => api::standard_error(status),
                        _ => {
                            dever_runtime::log::error("API handler failed", Vec::new());
                            api::internal_error()
                        }
                    }
                }
            }

            enum Inputs {
                Fields(api::Inputs),
                Multipart(api::upload::Multipart),
            }
            impl Inputs {
                fn field(
                    &mut self,
                    name: &str,
                    required: bool,
                    kind: api::QueryValue,
                ) -> Result<String, api::InputError> {
                    match (self, required) {
                        (Self::Fields(inputs), true) => inputs.json(name, kind),
                        (Self::Fields(inputs), false) => inputs.optional_json(name, kind),
                        (Self::Multipart(inputs), true) => inputs.json(name, kind),
                        (Self::Multipart(inputs), false) => inputs.optional_json(name, kind),
                    }
                }
                fn raw(
                    &mut self,
                    name: &str,
                    required: bool,
                ) -> Result<Option<String>, api::InputError> {
                    match (self, required) {
                        (Self::Fields(inputs), true) => inputs.raw_json(name).map(Some),
                        (Self::Fields(inputs), false) => inputs.optional_raw_json(name),
                        (Self::Multipart(inputs), true) => inputs.raw_json(name).map(Some),
                        (Self::Multipart(inputs), false) => inputs.optional_raw_json(name),
                    }
                }
                fn finish(self) -> Result<(), api::InputError> {
                    match self {
                        Self::Fields(inputs) => inputs.finish(),
                        Self::Multipart(inputs) => inputs.finish(),
                    }
                }
            }

            struct Authentication {
                function: Callable,
                pack: PackClaims,
                unpack: UnpackIdentity,
            }
            impl Authentication {
                unsafe fn new(raw: *const ApiAuth) -> Result<Self, managed::AbiFailure> {
                    let auth = unsafe { record(raw) }?;
                    Ok(Self {
                        function: unsafe {
                            Callable::new(
                                auth.asynchronous,
                                auth.synchronous,
                                auth.input_type,
                                auth.fault_status,
                            )
                        }?,
                        pack: auth
                            .pack
                            .ok_or_else(|| managed::invalid("missing claims pack callback"))?,
                        unpack: auth
                            .unpack
                            .ok_or_else(|| managed::invalid("missing identity unpack callback"))?,
                    })
                }

                async fn verify(
                    &self,
                    claims: &auth::Claims,
                ) -> Result<auth::Identity, ForeignFault> {
                    let mut input =
                        unsafe { Owned::new(self.function.input) }.map_err(ForeignFault::from)?;
                    {
                        let subject = managed::owned(claims.subject.clone());
                        let session = managed::owned(claims.session.clone());
                        let tenant = claims
                            .tenant
                            .clone()
                            .map_or(ptr::null_mut(), managed::owned);
                        let site = managed::owned(claims.site.clone());
                        let mut diagnostic = buffer();
                        let status = unsafe {
                            (self.pack)(
                                subject,
                                session,
                                tenant,
                                site,
                                input.pointer(),
                                &mut diagnostic,
                            )
                        };
                        unsafe {
                            managed::release::<String>(subject);
                            managed::release::<String>(session);
                            managed::release::<String>(tenant);
                            managed::release::<String>(site);
                        }
                        // A successful pack owns a complete row even if its diagnostic is malformed.
                        input.initialized = status == 0;
                        callback(status, diagnostic).map_err(ForeignFault::from)?;
                    }
                    let output = self.function.invoke(input).await?;
                    let (mut id, mut user, mut has_user, mut tenant, mut has_tenant) =
                        (ptr::null_mut(), 0, 0, 0, 0);
                    let mut diagnostic = buffer();
                    let status = unsafe {
                        (self.unpack)(
                            output.pointer(),
                            &mut id,
                            &mut user,
                            &mut has_user,
                            &mut tenant,
                            &mut has_tenant,
                            &mut diagnostic,
                        )
                    };
                    let result = callback(status, diagnostic).and_then(|()| {
                        if has_user > 1 || has_tenant > 1 {
                            return Err("invalid identity presence flag".into());
                        }
                        unsafe { managed::borrowed::<String>(id) }
                            .map(|id| auth::Identity {
                                id: id.clone(),
                                user_id: (has_user == 1).then_some(user),
                                tenant_id: (has_tenant == 1).then_some(tenant),
                            })
                            .map_err(|error| error.message)
                    });
                    unsafe { managed::release::<String>(id) };
                    result.map_err(ForeignFault::from)
                }
            }

            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            #[repr(C)]
            pub struct JobDescriptor {
                target: Name,
                schema: Name,
                attempts: u32,
                timeout_ms: u32,
                schedule: *const Name,
                binding: *const Binding,
                components: *const Name,
                component_count: u64,
                handler: *const ApiHandler,
            }
            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            #[repr(C)]
            pub struct JobAuth {
                provider: Name,
                auth: *const ApiAuth,
            }
            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            struct CompiledJob {
                function: Callable,
                decode: Decode,
                components: Vec<&'static str>,
            }
            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            pub(super) struct JobRegistry {
                pub(super) application: Arc<application::Session>,
                pub(super) resources: Arc<dever_runtime::job::resources::Resources>,
                handlers: Vec<CompiledJob>,
                authentication: std::collections::BTreeMap<&'static str, Authentication>,
                permissions: std::collections::BTreeSet<(&'static str, &'static str)>,
                components: Vec<&'static str>,
            }
            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            pub(super) unsafe fn job_registry(
                raw: *const c_void,
            ) -> Result<Arc<JobRegistry>, managed::AbiFailure> {
                unsafe { managed::borrowed::<Arc<JobRegistry>>(raw) }.cloned()
            }
            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            impl JobRegistry {
                async fn require_components(
                    &self,
                    claim: &dever_runtime::job::store::Claim,
                ) -> Result<(), dever_runtime::job::store::Outcome> {
                    let Some(index) = self
                        .resources
                        .bindings
                        .iter()
                        .position(|binding| binding.spec.target == claim.target)
                    else {
                        return Ok(());
                    };
                    dever_runtime::job::require_components(
                        &claim.execution,
                        &self.handlers[index].components,
                        &self.components,
                    )
                    .await
                }

                async fn authorize(
                    &self,
                    claim: &dever_runtime::job::store::Claim,
                ) -> Result<(), dever_runtime::job::store::Outcome> {
                    use dever_runtime::job::{ExecutionIdentity, store::Outcome};
                    let ExecutionIdentity::User {
                        tenant_id,
                        provider,
                        site,
                        subject,
                        session,
                        tenant_claim,
                        permission_key,
                    } = &claim.execution
                    else {
                        return self.require_components(claim).await;
                    };
                    if self.permissions.is_empty()
                        || auth::validate_job_source(provider, site).is_err()
                    {
                        return Err(Outcome::InvalidIdentity);
                    }
                    let authentication = self
                        .authentication
                        .get(provider.as_str())
                        .ok_or(Outcome::InvalidIdentity)?;
                    let claims = auth::Claims {
                        subject: subject.clone(),
                        session: session.clone(),
                        tenant: tenant_claim.clone(),
                        site: site.clone(),
                    };
                    let identity = authentication.verify(&claims).await.map_err(|fault| {
                        let status = match &fault {
                            ForeignFault::Typed(value) => unsafe {
                                (authentication.function.fault_status)(value.pointer())
                            },
                            ForeignFault::Runtime(_) => 500,
                        };
                        if matches!(status, 401 | 403) {
                            Outcome::IdentityRejected
                        } else {
                            dever_runtime::log::error(
                                "Job identity verification failed",
                                Vec::new(),
                            );
                            Outcome::Retry
                        }
                    })?;
                    if auth::validate_job_identity(&claims, &identity).is_err()
                        || identity.tenant_id != *tenant_id
                        || !self
                            .permissions
                            .contains(&(permission_key.as_str(), site.as_str()))
                    {
                        return Err(Outcome::IdentityRejected);
                    }
                    self.require_components(claim).await?;
                    let user_id = identity.user_id.ok_or(Outcome::IdentityRejected)?;
                    let settings = self.application.settings();
                    let database = if tenant_id.is_some() {
                        let tenant = settings.tenant().ok_or(Outcome::InvalidIdentity)?;
                        dever_runtime::tenant::database(tenant.database())
                            .await
                            .map_err(|_| Outcome::Retry)?
                    } else {
                        let connection = settings
                            .tenant()
                            .map(|tenant| tenant.database())
                            .unwrap_or("default");
                        self.application
                            .databases()
                            .database(Some(connection), "")
                            .map_err(|_| Outcome::Retry)?
                    };
                    match auth::store::authorize(database, user_id, site, permission_key).await {
                        Ok(true) => Ok(()),
                        Ok(false) => Err(Outcome::IdentityRejected),
                        Err(_) => Err(Outcome::Retry),
                    }
                }

                async fn dispatch(
                    &self,
                    claim: dever_runtime::job::store::Claim,
                ) -> dever_runtime::job::store::Outcome {
                    use dever_runtime::job::store::Outcome;
                    if let Err(outcome) = self.authorize(&claim).await {
                        return outcome;
                    }
                    let Some(index) = self
                        .resources
                        .bindings
                        .iter()
                        .position(|binding| binding.spec.target == claim.target)
                    else {
                        return Outcome::UnknownTarget;
                    };
                    if self.resources.bindings[index].spec.schema != claim.schema {
                        return Outcome::SchemaMismatch;
                    }
                    let handler = &self.handlers[index];
                    let mut input = match unsafe { Owned::new(handler.function.input) } {
                        Ok(input) => input,
                        Err(_) => return Outcome::Retry,
                    };
                    {
                        let payload = managed::owned(claim.payload);
                        let mut diagnostic = buffer();
                        let status = unsafe {
                            (handler.decode)(payload, 0, input.pointer(), &mut diagnostic)
                        };
                        unsafe { managed::release::<String>(payload) };
                        input.initialized = status == 0;
                        if callback(status, diagnostic).is_err() {
                            return if status == 1 {
                                Outcome::InvalidPayload
                            } else {
                                Outcome::Retry
                            };
                        }
                    }
                    match handler.function.invoke(input).await {
                        Ok(_) => Outcome::Success,
                        Err(_) => Outcome::Retry,
                    }
                }

                async fn serve(self: Arc<Self>) -> Result<(), String> {
                    if !dever_runtime::lifecycle::worker_enabled() {
                        return Ok(());
                    }
                    let bindings = self.resources.bindings.clone();
                    let clock = self.resources.clock.clone();
                    let settings = self.application.settings().jobs()?;
                    dever_runtime::job::worker::serve(bindings, clock, settings, move |claim| {
                        let registry = self.clone();
                        async move { Box::pin(registry.dispatch(claim)).await }
                    })
                    .await
                }
            }

            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            /// # Safety
            /// Compiler descriptors and their names are immutable and process-static.
            /// Session and Clock are borrowed; out receives a registry owned by this root.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_job_session_new(
                raw: *const c_void,
                clock: *const c_void,
                descriptors: *const JobDescriptor,
                count: u64,
                authentication: *const JobAuth,
                auth_count: u64,
                permissions: *const Permission,
                permission_count: u64,
                components: *const Name,
                component_count: u64,
                out: *mut *mut c_void,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, error_buffer) } {
                    return status;
                }
                let result = (|| {
                    let application = unsafe { session(raw) }?;
                    let clock =
                        unsafe { managed::borrowed::<dever_runtime::time::Clock>(clock) }?.clone();
                    let mut bindings = Vec::new();
                    let mut handlers = Vec::new();
                    let mut targets = std::collections::BTreeSet::new();
                    for descriptor in unsafe { array(descriptors, count) }? {
                        let target = unsafe { name(&descriptor.target) }?;
                        if !targets.insert(target) {
                            return Err(managed::invalid("duplicate compiled Job target"));
                        }
                        let binding = unsafe { record(descriptor.binding) }?;
                        let scope = if flag(binding.tenant)? {
                            dever_runtime::database::ModelScope::Tenant
                        } else {
                            dever_runtime::database::ModelScope::Global
                        };
                        let storage = dever_runtime::database::StorageBinding::new(
                            if binding.explicit.is_null() {
                                None
                            } else {
                                Some(unsafe { name(record(binding.explicit)?) }?)
                            },
                            unsafe { name(&binding.root) }?,
                            scope,
                        );
                        let spec = dever_runtime::job::Spec {
                            target,
                            schema: unsafe { name(&descriptor.schema) }?,
                            attempts: descriptor.attempts,
                            timeout_ms: descriptor.timeout_ms,
                        };
                        let schedule = if descriptor.schedule.is_null() {
                            None
                        } else {
                            Some(
                                dever_runtime::cron::Cron::parse(unsafe {
                                    name(record(descriptor.schedule)?)
                                }?)
                                .map_err(managed::invalid)?,
                            )
                        };
                        let handler = unsafe { record(descriptor.handler) }?;
                        let function = unsafe {
                            Callable::new(
                                handler.asynchronous,
                                handler.synchronous,
                                handler.input_type,
                                handler.fault_status,
                            )
                        }?;
                        if function.output.size != 0 || function.output.align != 1 {
                            return Err(managed::invalid("Job handler must return Unit"));
                        }
                        handlers.push(CompiledJob {
                            function,
                            decode: handler
                                .decode
                                .ok_or_else(|| managed::invalid("missing Job payload decoder"))?,
                            components: unsafe {
                                names(descriptor.components, descriptor.component_count)
                            }?,
                        });
                        bindings.push(dever_runtime::job::worker::Binding::owned(
                            storage, scope, spec, schedule,
                        ));
                    }
                    let mut auth = std::collections::BTreeMap::new();
                    for descriptor in unsafe { array(authentication, auth_count) }? {
                        let provider = unsafe { name(&descriptor.provider) }?;
                        if auth
                            .insert(provider, unsafe { Authentication::new(descriptor.auth) }?)
                            .is_some()
                        {
                            return Err(managed::invalid("duplicate Job authentication provider"));
                        }
                    }
                    let permissions = unsafe { array(permissions, permission_count) }?
                        .iter()
                        .map(|permission| {
                            Ok((unsafe { name(&permission.key) }?, unsafe {
                                name(&permission.site)
                            }?))
                        })
                        .collect::<Result<_, managed::AbiFailure>>()?;
                    let components = unsafe { names(components, component_count) }?;
                    let resources =
                        Arc::new(dever_runtime::job::resources::Resources { bindings, clock });
                    application::with_resources(Some(application.clone()), || {
                        resources.validate(dever_runtime::lifecycle::worker_enabled())
                    })
                    .map_err(managed::runtime_error)?;
                    application
                        .initialize_jobs(resources.clone())
                        .map_err(managed::runtime_error)?;
                    Ok(Arc::new(JobRegistry {
                        application,
                        resources,
                        handlers,
                        authentication: auth,
                        permissions,
                        components,
                    }))
                })();
                unsafe { managed::complete_handle(out, error_buffer, result) }
            }

            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            /// # Safety
            /// Transfers one registry after application tasks have drained.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_job_session_release(raw: *mut c_void) {
                unsafe { managed::release::<Arc<JobRegistry>>(raw) };
            }

            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            /// # Safety
            /// Registry is borrowed and retained until this operation completes or is cancelled.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_job_drain(
                raw: *const c_void,
                limit: i64,
                error_buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(error_buffer) } {
                    return ptr::null_mut();
                }
                let registry = match unsafe { job_registry(raw) } {
                    Ok(registry) => registry,
                    Err(failure) => {
                        unsafe { error(error_buffer, failure.status, failure.message) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        let application = registry.application.clone();
                        application
                            .scope(async move {
                                let bindings = registry.resources.bindings.clone();
                                let clock = registry.resources.clock.clone();
                                dever_runtime::job::worker::drain(
                                    bindings,
                                    clock,
                                    limit,
                                    move |claim| {
                                        let registry = registry.clone();
                                        async move { Box::pin(registry.dispatch(claim)).await }
                                    },
                                )
                                .await
                            })
                            .await
                            .map(|count| OpValue::Database(database::Output::Int(count)))
                            .map_err(ForeignFault::from)
                    },
                    Some(OutputSlots::Int),
                )
            }

            /// # Safety
            /// Borrows the installed Session; booleans select compiler-owned service roots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_job_configure(
                raw: *const c_void,
                api: u8,
                worker: u8,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if !unsafe { empty(error_buffer) } {
                    return 2;
                }
                let result = (|| {
                    let session = unsafe { session(raw) }?;
                    let api = flag(api)?;
                    let worker = flag(worker)?;
                    application::with_resources(Some(session), || {
                        dever_runtime::lifecycle::configure(api, worker)
                    })
                    .map_err(managed::runtime_error)
                })();
                match result {
                    Ok(()) => 0,
                    Err(failure) => unsafe { error(error_buffer, failure.status, failure.message) },
                }
            }

            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            /// # Safety
            /// Process-static callback metadata. The compiler alone selects System roots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_job_system_scope(
                tenant: i64,
                has_tenant: u8,
                function: *const AsyncFunction,
                input: *const c_void,
                error_buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(error_buffer) } {
                    return ptr::null_mut();
                }
                let prepared = (|| {
                    let tenant = flag(has_tenant)?.then_some(tenant);
                    if tenant.is_some_and(|tenant| tenant <= 0) {
                        return Err(managed::invalid("invalid System tenant"));
                    }
                    let function = unsafe { record(function) }?;
                    let (_, output) =
                        unsafe { owned_layout(function.output_type) }.map_err(managed::invalid)?;
                    let (_, fault) =
                        unsafe { owned_layout(function.fault_type) }.map_err(managed::invalid)?;
                    let future =
                        unsafe { ForeignFuture::new(function, input) }.map_err(managed::invalid)?;
                    Ok((
                        tenant,
                        future,
                        OutputSlots::Typed {
                            align: output.align(),
                            fault_align: fault.align(),
                        },
                    ))
                })();
                let (tenant, future, slots) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { error(error_buffer, failure.status, failure.message) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        let execution = async move {
                            match tenant {
                                Some(tenant) => dever_runtime::tenant::scope(tenant, future).await,
                                None => future.await,
                            }
                        };
                        dever_runtime::job::scope_system(tenant, execution)
                            .await
                            .map(OpValue::Value)
                    },
                    Some(slots),
                )
            }

            struct CompiledRoute {
                method: &'static str,
                path: &'static str,
                directory: Vec<&'static str>,
                components: Vec<&'static str>,
                permission: Option<(&'static str, &'static str)>,
                auth: Authentication,
                function: Callable,
                decode: Decode,
                fault_message: Option<FaultMessage>,
                anonymous: bool,
                multipart: bool,
                detail: bool,
            }
            impl CompiledRoute {
                unsafe fn new(row: &Route) -> Result<Self, managed::AbiFailure> {
                    let anonymous = flag(row.anonymous)?;
                    if row.auth.is_null() || (!anonymous && row.permission.is_null()) {
                        return Err(managed::invalid(
                            "API route is missing authentication or permission metadata",
                        ));
                    }
                    let handler = unsafe { record(row.handler) }?;
                    let function = unsafe {
                        Callable::new(
                            handler.asynchronous,
                            handler.synchronous,
                            handler.input_type,
                            handler.fault_status,
                        )
                    }?;
                    if function.output.size != size_of::<*mut c_void>() as u64
                        || function.output.align != align_of::<*mut c_void>() as u64
                    {
                        return Err(managed::invalid("API handler must return one owned Text"));
                    }
                    let auth = unsafe { Authentication::new(row.auth) }?;
                    let permission = if row.permission.is_null() {
                        None
                    } else {
                        let permission = unsafe { record(row.permission) }?;
                        Some((unsafe { name(&permission.key) }?, unsafe {
                            name(&permission.site)
                        }?))
                    };
                    Ok(Self {
                        method: unsafe { name(&row.method) }?,
                        path: unsafe { name(&row.path) }?,
                        directory: unsafe { names(row.directory, row.directory_count) }?,
                        components: unsafe { names(row.components, row.component_count) }?,
                        permission,
                        auth,
                        function,
                        decode: handler
                            .decode
                            .ok_or_else(|| managed::invalid("missing API input decoder"))?,
                        fault_message: handler.fault_message,
                        anonymous,
                        multipart: flag(row.multipart)?,
                        detail: flag(row.detail)?,
                    })
                }

                async fn identity(
                    &self,
                    prepared: &auth::Prepared,
                ) -> Result<Option<auth::Identity>, http::Response> {
                    let Some(claims) = prepared.claims() else {
                        return Ok(None);
                    };
                    self.auth
                        .verify(claims)
                        .await
                        .map(Some)
                        .map_err(|error| self.auth.function.failure(error, None))
                }

                async fn call(
                    &self,
                    mut request: http::Request,
                    detail: i64,
                    tenant_components: &[&str],
                ) -> http::Response {
                    #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
                    {
                        if let Err(error) =
                            auth::require_components(&self.components, tenant_components).await
                        {
                            return api::auth_error(error);
                        }
                        if let Some((permission, site)) = self.permission
                            && let Err(error) = auth::authorize_permission(permission, site).await
                        {
                            return api::auth_error(error);
                        }
                    }
                    #[cfg(not(any(feature = "runtime-sqlite", feature = "runtime-postgres")))]
                    {
                        let _ = tenant_components;
                        if self.permission.is_some()
                            || (!self.components.is_empty()
                                && config::current_settings().tenant().is_some())
                        {
                            return api::internal_error();
                        }
                    }
                    if !self.multipart
                        && let Err(error) = api::upload::read_body(&mut request).await
                    {
                        return api::invalid_input(error);
                    }
                    let detail = if self.detail {
                        let path = request.target.split('?').next().unwrap_or("");
                        match path
                            .strip_prefix(self.path)
                            .and_then(|suffix| suffix.strip_prefix('/'))
                            .and_then(|id| id.parse::<i64>().ok())
                            .filter(|id| *id > 0)
                        {
                            Some(id) => id,
                            None => return api::not_found(),
                        }
                    } else {
                        detail
                    };
                    let inputs = if self.multipart {
                        api::upload::read_multipart(&request)
                            .await
                            .map(Inputs::Multipart)
                    } else {
                        api::Inputs::from_request(&request).map(Inputs::Fields)
                    };
                    let mut inputs = match inputs {
                        Ok(inputs) => inputs,
                        Err(error) => return api::invalid_input(error),
                    };
                    let mut input = match unsafe { Owned::new(self.function.input) } {
                        Ok(input) => input,
                        Err(_) => return api::internal_error(),
                    };
                    {
                        let mut diagnostic = buffer();
                        let status = unsafe {
                            (self.decode)(
                                (&mut inputs as *mut Inputs).cast(),
                                detail,
                                input.pointer(),
                                &mut diagnostic,
                            )
                        };
                        if let Err(message) = callback(status, diagnostic) {
                            return if status == 1 {
                                api::invalid_input(api::InputError(message))
                            } else {
                                api::internal_error()
                            };
                        }
                    }
                    input.initialized = true;
                    if let Err(error) = inputs.finish() {
                        return api::invalid_input(error);
                    }
                    let output = match self.function.invoke(input).await {
                        Ok(output) => output,
                        Err(error) => return self.function.failure(error, self.fault_message),
                    };
                    let json = unsafe { *output.pointer().cast::<*mut c_void>() };
                    let json = match unsafe { managed::borrowed::<String>(json) } {
                        Ok(json) => json,
                        Err(_) => return api::internal_error(),
                    };
                    let mut encoder = dever_runtime::wire::Encoder::default();
                    let value = match encoder.json(json).and_then(|()| encoder.finish()) {
                        Ok(value) => value,
                        Err(_) => return api::internal_error(),
                    };
                    if api::commit_response_metadata().is_err() {
                        return api::internal_error();
                    }
                    api::success(value)
                }

                async fn dispatch(
                    &self,
                    request: http::Request,
                    detail: i64,
                    tenant_components: &[&str],
                ) -> http::Response {
                    let prepared = match auth::prepare(&self.directory, &request, self.anonymous) {
                        Ok(prepared) => prepared,
                        Err(error) => return api::auth_error(error),
                    };
                    let identity = match self.identity(&prepared).await {
                        Ok(identity) => identity,
                        Err(response) => return response,
                    };
                    // Keep nested identity/tenant scopes from repeatedly embedding the request future.
                    let handler = Box::pin(self.call(request, detail, tenant_components));
                    match auth::scope(prepared, identity, handler).await {
                        Ok(response) => response,
                        Err(error) => api::auth_error(error),
                    }
                }
            }

            struct Routes {
                routes: Vec<CompiledRoute>,
                tenant_components: Vec<&'static str>,
            }
            impl Routes {
                async fn dispatch(&self, request: http::Request) -> Result<http::Response, String> {
                    let path = request.target.split('?').next().unwrap_or("");
                    let mut allowed = Vec::new();
                    for route in &self.routes {
                        let suffix = if route.detail {
                            path.strip_prefix(route.path)
                                .and_then(|suffix| suffix.strip_prefix('/'))
                        } else if path == route.path {
                            Some("")
                        } else {
                            None
                        };
                        let Some(_) = suffix else {
                            continue;
                        };
                        if route.method != request.method {
                            allowed.push(route.method);
                            continue;
                        }
                        return Ok(route.dispatch(request, 0, &self.tenant_components).await);
                    }
                    Ok(if allowed.is_empty() {
                        api::not_found()
                    } else {
                        api::method_not_allowed(&allowed)
                    })
                }
            }

            /// # Safety
            /// Route descriptors and all callback/type/name metadata are process-static.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_serve(
                raw: *const c_void,
                routes: *const Route,
                count: u64,
                tenant_components: *const Name,
                component_count: u64,
                error_buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(error_buffer) } {
                    return ptr::null_mut();
                }
                let prepared = (|| {
                    let session = unsafe { session(raw) }?;
                    let routes = unsafe { array(routes, count) }?
                        .iter()
                        .map(|route| unsafe { CompiledRoute::new(route) })
                        .collect::<Result<Vec<_>, _>>()?;
                    let tenant_components = unsafe { names(tenant_components, component_count) }?;
                    Ok::<_, managed::AbiFailure>((
                        session,
                        Arc::new(Routes {
                            routes,
                            tenant_components,
                        }),
                    ))
                })();
                let (session, routes) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { error(error_buffer, failure.status, failure.message) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        session
                            .scope(async move {
                                dever_runtime::lifecycle::configure(true, false)?;
                                dever_runtime::lifecycle::run(api::serve(move |request| {
                                    let routes = routes.clone();
                                    async move { routes.dispatch(request).await }
                                }))
                                .await
                            })
                            .await
                            .map(|()| OpValue::Unit)
                            .map_err(ForeignFault::from)
                    },
                    Some(OutputSlots::Unit),
                )
            }

            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            /// # Safety
            /// Static route metadata; both handles belong to the same configured Session.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_serve_with_jobs(
                raw: *const c_void,
                routes: *const Route,
                count: u64,
                tenant_components: *const Name,
                component_count: u64,
                jobs: *const c_void,
                error_buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(error_buffer) } {
                    return ptr::null_mut();
                }
                let prepared = (|| {
                    let session = unsafe { session(raw) }?;
                    let jobs = unsafe { job_registry(jobs) }?;
                    if !Arc::ptr_eq(&session, &jobs.application) {
                        return Err(managed::invalid("Job and API Sessions differ"));
                    }
                    let routes = unsafe { array(routes, count) }?
                        .iter()
                        .map(|route| unsafe { CompiledRoute::new(route) })
                        .collect::<Result<Vec<_>, _>>()?;
                    let components = unsafe { names(tenant_components, component_count) }?;
                    Ok((
                        session,
                        jobs,
                        Arc::new(Routes {
                            routes,
                            tenant_components: components,
                        }),
                    ))
                })();
                let (session, jobs, routes) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { error(error_buffer, failure.status, failure.message) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        session
                            .scope(dever_runtime::lifecycle::run(async move {
                                let has_api = !routes.routes.is_empty();
                                let api = async move {
                                    if !has_api {
                                        return Ok(());
                                    }
                                    api::serve(move |request| {
                                        let routes = routes.clone();
                                        async move { routes.dispatch(request).await }
                                    })
                                    .await
                                };
                                dever_runtime::lifecycle::serve_both(api, jobs.serve()).await
                            }))
                            .await
                            .map(|()| OpValue::Unit)
                            .map_err(ForeignFault::from)
                    },
                    Some(OutputSlots::Unit),
                )
            }

            /// # Safety
            /// Static permission rows and schema name are borrowed; operation owns session.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_initialize(
                raw: *const c_void,
                fingerprint: *const Name,
                permissions: *const Permission,
                count: u64,
                service_start: u8,
                error_buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(error_buffer) } {
                    return ptr::null_mut();
                }
                let prepared = (|| {
                    let session = unsafe { session(raw) }?;
                    let fingerprint = unsafe { name(record(fingerprint)?) }?.to_owned();
                    let service_start = flag(service_start)?;
                    let rows = unsafe { array(permissions, count) }?;
                    #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
                    let permissions = rows
                        .iter()
                        .map(|row| {
                            Ok(auth::store::Permission {
                                key: unsafe { name(&row.key) }?,
                                component: unsafe { name(&row.component) }?,
                                domain: unsafe { name(&row.domain) }?,
                                site: unsafe { name(&row.site) }?,
                                action: unsafe { name(&row.action) }?,
                                method: unsafe { name(&row.method) }?,
                            })
                        })
                        .collect::<Result<Vec<_>, managed::AbiFailure>>()?;
                    #[cfg(not(any(feature = "runtime-sqlite", feature = "runtime-postgres")))]
                    let permissions = rows.len();
                    session
                        .initialize_authorization(!rows.is_empty())
                        .map_err(managed::runtime_error)?;
                    Ok::<_, managed::AbiFailure>((session, fingerprint, permissions, service_start))
                })();
                let (session, fingerprint, permissions, service_start) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { error(error_buffer, failure.status, failure.message) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        session
                            .clone()
                            .scope(async move {
                                #[cfg(any(
                                    feature = "runtime-sqlite",
                                    feature = "runtime-postgres"
                                ))]
                                {
                                    session
                                        .databases()
                                        .prepare()
                                        .await
                                        .map_err(|error| error.to_string())?;
                                    if session.settings().tenant().is_some() {
                                        dever_runtime::tenant::initialize(&fingerprint)
                                            .await
                                            .map_err(|error| error.to_string())?;
                                    }
                                    if service_start && !permissions.is_empty() {
                                        let connection = session
                                            .settings()
                                            .tenant()
                                            .map(|tenant| tenant.database())
                                            .unwrap_or("default");
                                        let database = session
                                            .databases()
                                            .database(Some(connection), "")
                                            .map_err(|error| error.to_string())?;
                                        auth::store::sync_catalog(database.clone(), &permissions)
                                            .await
                                            .map_err(|error| error.to_string())?;
                                        auth::store::initialize_roles(database)
                                            .await
                                            .map_err(|error| error.to_string())?;
                                    }
                                }
                                #[cfg(not(any(
                                    feature = "runtime-sqlite",
                                    feature = "runtime-postgres"
                                )))]
                                {
                                    let _ = (fingerprint, service_start);
                                    if permissions != 0 || session.settings().tenant().is_some() {
                                        return Err(
                                            "API authorization requires a database runtime"
                                                .to_owned(),
                                        );
                                    }
                                }
                                Ok::<_, String>(())
                            })
                            .await
                            .map(|()| OpValue::Unit)
                            .map_err(ForeignFault::from)
                    },
                    Some(OutputSlots::Unit),
                )
            }

            type Upload = Mutex<Option<api::upload::Upload>>;
            /// # Safety
            /// Name is borrowed static UTF-8; nullable value borrows a Text owner.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_text_bounds(
                field: *const Name,
                value: *const c_void,
                minimum: i64,
                maximum: i64,
                has_maximum: u8,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if !unsafe { empty(error_buffer) } {
                    return 2;
                }
                let result = (|| {
                    let name = unsafe { name(record(field)?) }?;
                    let value = if value.is_null() {
                        None
                    } else {
                        Some(unsafe { managed::borrowed::<String>(value) }?.as_str())
                    };
                    let minimum = u32::try_from(minimum)
                        .map_err(|_| managed::invalid("invalid text minimum"))?;
                    let maximum = if flag(has_maximum)? {
                        Some(
                            u32::try_from(maximum)
                                .map_err(|_| managed::invalid("invalid text maximum"))?,
                        )
                    } else {
                        None
                    };
                    api::text_bounds(name, value, minimum, maximum)
                        .map_err(|error| managed::runtime_error(error.to_string()))
                })();
                match result {
                    Ok(()) => 0,
                    Err(failure) => unsafe { error(error_buffer, failure.status, failure.message) },
                }
            }
            /// # Safety
            /// All integer outputs and error are distinct aligned writable slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_pagination(
                page: i64,
                size: i64,
                maximum: i64,
                out_page: *mut i64,
                out_size: *mut i64,
                out_offset: *mut i64,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out_page, error_buffer) } {
                    return status;
                }
                if out_size.is_null()
                    || !out_size.is_aligned()
                    || out_offset.is_null()
                    || !out_offset.is_aligned()
                {
                    return unsafe { error(error_buffer, 2, "invalid pagination output") };
                }
                let maximum = match usize::try_from(maximum) {
                    Ok(value) => value,
                    Err(_) => return unsafe { error(error_buffer, 2, "invalid page maximum") },
                };
                match dever_runtime::orm::pagination(page, size, maximum) {
                    Ok((page, size, offset)) => {
                        unsafe {
                            out_page.write(page);
                            out_size.write(size);
                            out_offset.write(offset);
                        };
                        0
                    }
                    Err(failure) => unsafe { error(error_buffer, 1, failure.to_string()) },
                }
            }
            /// # Safety
            /// Upload is a live matching owner or null. This alias cannot consume twice.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_upload_retain(raw: *const c_void) -> *mut c_void {
                unsafe { managed::retain::<Upload>(raw) }
            }
            /// # Safety
            /// Transfers a matching Upload owner or null.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_upload_release(raw: *mut c_void) {
                unsafe { managed::release::<Upload>(raw) }
            }
            unsafe fn with_upload<T>(
                raw: *const c_void,
                read: impl FnOnce(&api::upload::Upload) -> T,
            ) -> Result<T, managed::AbiFailure> {
                let upload = unsafe { managed::borrowed::<Upload>(raw) }?
                    .lock()
                    .map_err(|_| managed::runtime_error("upload state lock failed"))?;
                upload
                    .as_ref()
                    .map(read)
                    .ok_or_else(|| managed::runtime_error("upload was already consumed"))
            }
            unsafe fn take_upload(
                raw: *mut c_void,
            ) -> Result<api::upload::Upload, managed::AbiFailure> {
                let result = unsafe { managed::borrowed::<Upload>(raw) }.and_then(|owner| {
                    owner
                        .lock()
                        .map_err(|_| managed::runtime_error("upload state lock failed"))?
                        .take()
                        .ok_or_else(|| managed::runtime_error("upload was already consumed"))
                });
                unsafe { managed::release::<Upload>(raw) };
                result
            }
            macro_rules! upload_read {
                ($name:ident, $method:ident, $out:ty, $complete:ident) => {
                    /// # Safety
                    /// Upload is borrowed; output/error are distinct writable slots.
                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn $name(
                        raw: *const c_void,
                        out: *mut $out,
                        error_buffer: *mut Buffer,
                    ) -> u32 {
                        if let Err(status) = unsafe { managed::checked_output(out, error_buffer) } {
                            return status;
                        }
                        let result = unsafe { with_upload(raw, api::upload::Upload::$method) };
                        unsafe { managed::$complete(out, error_buffer, result) }
                    }
                };
            }
            upload_read!(
                dever_rt_v1_upload_filename,
                filename,
                *mut c_void,
                complete_handle
            );
            upload_read!(
                dever_rt_v1_upload_content_type,
                content_type,
                *mut c_void,
                complete_handle
            );
            upload_read!(dever_rt_v1_upload_size, size, i64, complete);
            /// # Safety
            /// Consumes one Upload owner, including when the output metadata is invalid.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_upload_close_take(
                raw: *mut c_void,
                out: *mut u8,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, error_buffer) } {
                    unsafe { managed::release::<Upload>(raw) };
                    return status;
                }
                let result = unsafe { take_upload(raw) }.map(|upload| {
                    upload.close();
                    0
                });
                unsafe { managed::complete(out, error_buffer, result) }
            }
            /// # Safety
            /// Consumes one Upload owner; pending cancellation owns and drops the resource.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_upload_store_take(
                raw: *mut c_void,
                error_buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(error_buffer) } {
                    unsafe { managed::release::<Upload>(raw) };
                    return ptr::null_mut();
                }
                let upload = match unsafe { take_upload(raw) } {
                    Ok(upload) => upload,
                    Err(failure) => {
                        unsafe { error(error_buffer, failure.status, failure.message) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        api::upload::store(upload)
                            .await
                            .map(|value| OpValue::Api(Output::UploadId(value)))
                            .map_err(ForeignFault::from)
                    },
                    Some(OutputSlots::Handle),
                )
            }

            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            mod roles {
                use super::*;
                type PackPermission = unsafe extern "C" fn(
                    *const c_void,
                    *const c_void,
                    *const c_void,
                    *const c_void,
                    *const c_void,
                    *const c_void,
                    *mut c_void,
                );
                /// # Safety
                /// Descriptor and callback are static and initialize one complete permission row.
                #[unsafe(no_mangle)]
                pub unsafe extern "C" fn dever_rt_v1_auth_permissions(
                    element: *const managed::TypeDescriptor,
                    pack: Option<PackPermission>,
                    error_buffer: *mut Buffer,
                ) -> *mut AsyncOp {
                    if !unsafe { empty(error_buffer) } {
                        return ptr::null_mut();
                    }
                    let prepared = (|| {
                        Ok::<_, managed::AbiFailure>((
                            unsafe { managed::descriptor(element, false) }?,
                            pack.ok_or_else(|| {
                                managed::invalid("missing permission pack callback")
                            })?,
                        ))
                    })();
                    let (element, pack) = match prepared {
                        Ok(value) => value,
                        Err(failure) => {
                            unsafe { error(error_buffer, failure.status, failure.message) };
                            return ptr::null_mut();
                        }
                    };
                    operation_with_output(
                        async move {
                            let permissions =
                                auth::permissions().await.map_err(ForeignFault::from)?;
                            let mut values = Vec::with_capacity(permissions.len());
                            for permission in permissions {
                                let texts = [
                                    permission.key,
                                    permission.component,
                                    permission.domain,
                                    permission.site,
                                    permission.action,
                                    permission.method,
                                ]
                                .map(managed::owned);
                                let value = Element::from_initializer(element, |out| unsafe {
                                    pack(
                                        texts[0], texts[1], texts[2], texts[3], texts[4], texts[5],
                                        out,
                                    )
                                });
                                for text in texts {
                                    unsafe { managed::release::<String>(text) };
                                }
                                values.push(value);
                            }
                            Ok(OpValue::Api(Output::Permissions(managed::ListHandle {
                                values: dever_runtime::collections::List::new(values),
                                element,
                            })))
                        },
                        Some(OutputSlots::Handle),
                    )
                }
                macro_rules! role_op {
                    ($name:ident($($arg:ident:$ty:ty),*), $prepare:expr, $run:expr) => {
                        /// # Safety
                        /// Input handles are borrowed matching owners and cloned before return.
                        #[unsafe(no_mangle)]
                        pub unsafe extern "C" fn $name($($arg:$ty,)*error_buffer:*mut Buffer)->*mut AsyncOp {
                            if !unsafe {empty(error_buffer)} {return ptr::null_mut();}
                            let prepared:Result<_,managed::AbiFailure> = $prepare;
                            let prepared=match prepared {Ok(value)=>value,Err(failure)=>{unsafe {error(error_buffer,failure.status,failure.message)};return ptr::null_mut();}};
                            operation_with_output(async move {($run)(prepared).await.map(|()|OpValue::Unit).map_err(ForeignFault::from)},Some(OutputSlots::Unit))
                        }
                    };
                }
                unsafe fn text(raw: *const c_void) -> Result<String, managed::AbiFailure> {
                    unsafe { managed::borrowed::<String>(raw) }.cloned()
                }
                role_op!(dever_rt_v1_auth_save_role(id:*const c_void,name:*const c_void,all:u8,permissions:*const c_void),(|| {
                    let permissions=unsafe {managed::borrowed::<managed::ListHandle>(permissions)}?;
                    if permissions.element.size!=size_of::<*mut c_void>() || permissions.element.align!=align_of::<*mut c_void>() {return Err(managed::invalid("role permissions require Text rows"));}
                    let values=permissions.values.values().iter().map(|value|unsafe {text(*value.pointer().cast::<*mut c_void>())}).collect::<Result<Vec<_>,_>>()?;
                    Ok((unsafe {text(id)}?,unsafe {text(name)}?,flag(all)?,values))
                })(),|(id,name,all,permissions):(String,String,bool,Vec<String>)|async move {auth::save_role(&id,&name,all,permissions).await});
                role_op!(dever_rt_v1_auth_grant_role(user:i64,role:*const c_void),unsafe {text(role)},|role:String|async move {auth::grant_role(user,&role).await});
                role_op!(dever_rt_v1_auth_revoke_role(user:i64,role:*const c_void),unsafe {text(role)},|role:String|async move {auth::revoke_role(user,&role).await});
                role_op!(dever_rt_v1_auth_disable_role(role:*const c_void),unsafe {text(role)},|role:String|async move {auth::disable_role(&role).await});
                role_op!(dever_rt_v1_api_tenant_owner(raw:*const c_void,tenant:i64,site:*const c_void,user:i64),(||Ok((unsafe {session(raw)}?,unsafe {text(site)}?)))(),|(session,site):(Arc<application::Session>,String)|async move {
                    if tenant<=0 || user<=0 {return Err("tenant id and user id must be positive".into());}
                    if !session.settings().sites().contains_key(&site) {return Err(format!("unknown site '{site}'"));}
                    let connection=session.settings().tenant().ok_or("tenant storage is not configured")?.database().to_owned();
                    session.scope(dever_runtime::tenant::scope(tenant,async move {
                        let database=dever_runtime::tenant::database(&connection).await.map_err(|error|error.to_string())?;
                        auth::store::initialize_roles(database.clone()).await.map_err(|error|error.to_string())?;
                        auth::store::provision_owner(database,user,&site).await.map_err(|error|error.to_string())?;
                        Ok(())
                    })).await
                });
                role_op!(dever_rt_v1_api_tenant_component(raw:*const c_void,tenant:i64,component:*const c_void,enabled:u8,manifest:*const Name,count:u64),(||Ok((unsafe {session(raw)}?,unsafe {text(component)}?,flag(enabled)?,unsafe {names(manifest,count)}?)))(),|(session,component,enabled,manifest):(Arc<application::Session>,String,bool,Vec<&'static str>)|async move {
                    session.scope(async move {
                        let result=if enabled {dever_runtime::tenant::enable_component(tenant,&component,&manifest).await} else {dever_runtime::tenant::disable_component(tenant,&component,&manifest).await};
                        result.map_err(|error|error.to_string())
                    }).await
                });
            }
            unsafe fn inputs<'a>(raw: *mut c_void) -> Result<&'a mut Inputs, managed::AbiFailure> {
                if raw.is_null() || !raw.cast::<Inputs>().is_aligned() {
                    return Err(managed::invalid("invalid API inputs"));
                }
                Ok(unsafe { &mut *raw.cast::<Inputs>() })
            }
            /// # Safety
            /// Inputs is borrowed exclusively during the synchronous decode callback.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_input_field(
                raw: *mut c_void,
                field: *const Name,
                required: u8,
                kind: u32,
                out: *mut *mut c_void,
                present: *mut u8,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, error_buffer) } {
                    return status;
                }
                if present.is_null() {
                    return unsafe { error(error_buffer, 2, "missing API input presence") };
                }
                let result = (|| {
                    let kind = match kind {
                        0 => api::QueryValue::Text,
                        1 => api::QueryValue::Int,
                        2 => api::QueryValue::Bool,
                        3 => api::QueryValue::Float,
                        _ => return Err(managed::invalid("invalid query value kind")),
                    };
                    let value = unsafe { inputs(raw) }?
                        .field(unsafe { name(record(field)?) }?, flag(required)?, kind)
                        .map_err(|error| managed::runtime_error(error.to_string()))?;
                    unsafe { present.write(1) };
                    Ok(value)
                })();
                unsafe { managed::complete_handle(out, error_buffer, result) }
            }
            /// # Safety
            /// Same borrowed input and distinct output contract as api_input_field.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_input_raw_field(
                raw: *mut c_void,
                field: *const Name,
                required: u8,
                out: *mut *mut c_void,
                present: *mut u8,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, error_buffer) } {
                    return status;
                }
                if present.is_null() {
                    return unsafe { error(error_buffer, 2, "missing API input presence") };
                }
                let result = (|| {
                    unsafe { inputs(raw) }?
                        .raw(unsafe { name(record(field)?) }?, flag(required)?)
                        .map_err(|error| managed::runtime_error(error.to_string()))
                })();
                match result {
                    Ok(value) => {
                        unsafe {
                            present.write(u8::from(value.is_some()));
                            if let Some(value) = value {
                                out.write(managed::owned(value));
                            }
                        };
                        0
                    }
                    Err(failure) => unsafe { error(error_buffer, failure.status, failure.message) },
                }
            }
            /// # Safety
            /// Inputs is borrowed exclusively; out receives one affine Upload owner.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_input_upload(
                raw: *mut c_void,
                field: *const Name,
                out: *mut *mut c_void,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, error_buffer) } {
                    return status;
                }
                let result = (|| {
                    let name = unsafe { name(record(field)?) }?;
                    match unsafe { inputs(raw) }? {
                        Inputs::Multipart(inputs) => inputs
                            .take(name)
                            .map(|upload| Mutex::new(Some(upload)))
                            .map_err(|error| managed::runtime_error(error.to_string())),
                        Inputs::Fields(_) => {
                            Err(managed::invalid("Upload requires multipart inputs"))
                        }
                    }
                })();
                unsafe { managed::complete_handle(out, error_buffer, result) }
            }

            /// # Safety
            /// Binding metadata is borrowed; out receives one installed application owner.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_session_new(
                bindings: *const Binding,
                count: u64,
                transactions: *const TransactionBindings,
                transaction_count: u64,
                out: *mut *mut c_void,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, error_buffer) } {
                    return status;
                }
                let result = (|| {
                    let binding = |row: &Binding| -> Result<_, managed::AbiFailure> {
                        Ok((
                            if row.explicit.is_null() {
                                None
                            } else {
                                Some(unsafe { name(record(row.explicit)?) }?)
                            },
                            unsafe { name(&row.root) }?,
                            flag(row.tenant)?,
                        ))
                    };
                    let bindings = unsafe { array(bindings, count) }?
                        .iter()
                        .map(binding)
                        .collect::<Result<Vec<_>, _>>()?;
                    let transactions = unsafe { array(transactions, transaction_count) }?
                        .iter()
                        .map(|row| {
                            unsafe { array(row.bindings, row.count) }?
                                .iter()
                                .map(binding)
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let transactions = transactions.iter().map(Vec::as_slice).collect::<Vec<_>>();
                    let session = application::Session::load(
                        config::RuntimeProfile {
                            sqlite: cfg!(feature = "runtime-sqlite"),
                            postgres: cfg!(feature = "runtime-postgres"),
                        },
                        &bindings,
                        &transactions,
                    )
                    .map_err(managed::runtime_error)?;
                    application::install(session.clone()).map_err(managed::runtime_error)?;
                    Ok(session)
                })();
                unsafe { managed::complete_handle(out, error_buffer, result) }
            }
            /// # Safety
            /// Transfers the application owner after root cleanup.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_session_release(raw: *mut c_void) {
                unsafe { managed::release::<Arc<application::Session>>(raw) };
            }

            /// # Safety
            /// Borrows the installed application and initializes a Bool output.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_has_tenant(
                raw: *const c_void,
                out: *mut u8,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, error_buffer) } {
                    return status;
                }
                let result = unsafe { session(raw) }
                    .map(|session| u8::from(session.settings().tenant().is_some()));
                unsafe { managed::complete(out, error_buffer, result) }
            }

            /// # Safety
            /// Static typed callback metadata; borrowed input is cloned by frame creation.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_tenant_scope(
                tenant: i64,
                function: *const AsyncFunction,
                input: *const c_void,
                error_buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(error_buffer) } {
                    return ptr::null_mut();
                }
                let prepared = (|| {
                    if tenant <= 0 {
                        return Err(managed::runtime_error("tenant id must be positive"));
                    }
                    let function = unsafe { record(function) }?;
                    let (_, output) =
                        unsafe { owned_layout(function.output_type) }.map_err(managed::invalid)?;
                    let (_, fault) =
                        unsafe { owned_layout(function.fault_type) }.map_err(managed::invalid)?;
                    let future =
                        unsafe { ForeignFuture::new(function, input) }.map_err(managed::invalid)?;
                    Ok((
                        future,
                        OutputSlots::Typed {
                            align: output.align(),
                            fault_align: fault.align(),
                        },
                    ))
                })();
                let (future, slots) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { error(error_buffer, failure.status, failure.message) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
                        {
                            dever_runtime::tenant::scope(tenant, future)
                                .await
                                .map(OpValue::Value)
                        }
                        #[cfg(not(any(feature = "runtime-sqlite", feature = "runtime-postgres")))]
                        {
                            drop(future);
                            Err(ForeignFault::Runtime(
                                "tenant execution requires a database runtime".into(),
                            ))
                        }
                    },
                    Some(slots),
                )
            }

            /// # Safety
            /// Session is borrowed; compiler-owned component names live for this call.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_require_components(
                raw: *const c_void,
                components: *const Name,
                count: u64,
                manifest: *const Name,
                manifest_count: u64,
                error_buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(error_buffer) } {
                    return ptr::null_mut();
                }
                let prepared = (|| {
                    Ok::<_, managed::AbiFailure>((
                        unsafe { session(raw) }?,
                        unsafe { names(components, count) }?,
                        unsafe { names(manifest, manifest_count) }?,
                    ))
                })();
                let (session, components, manifest) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { error(error_buffer, failure.status, failure.message) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        if session.settings().tenant().is_none() || components.is_empty() {
                            return Ok(OpValue::Unit);
                        }
                        #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
                        {
                            let id = dever_runtime::tenant::current_id()
                                .map_err(|error| ForeignFault::Runtime(error.to_string()))?;
                            session
                                .scope(dever_runtime::tenant::require_components(
                                    id,
                                    &components,
                                    &manifest,
                                ))
                                .await
                                .map_err(|error| ForeignFault::Runtime(error.to_string()))?;
                            Ok(OpValue::Unit)
                        }
                        #[cfg(not(any(feature = "runtime-sqlite", feature = "runtime-postgres")))]
                        {
                            let _ = manifest;
                            Err(ForeignFault::Runtime(
                                "tenant components require a database runtime".into(),
                            ))
                        }
                    },
                    Some(OutputSlots::Unit),
                )
            }

            #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
            /// # Safety
            /// Borrows the application and returns an owned alias of its database Session.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_session_database(
                raw: *const c_void,
                out: *mut *mut c_void,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, error_buffer) } {
                    return status;
                }
                let result = unsafe { session(raw) }.map(|session| {
                    Arc::into_raw(session.databases().clone())
                        .cast_mut()
                        .cast::<c_void>()
                });
                unsafe { managed::complete(out, error_buffer, result) }
            }

            /// # Safety
            /// The callback descriptors are static. All writable outputs are distinct,
            /// and session_slot remains live until same-runtime cleanup finishes.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_application_root(
                function: *const AsyncFunction,
                input: *const c_void,
                out: *mut c_void,
                fault: *mut c_void,
                session_slot: *const *mut c_void,
                append_cause: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
                error_buffer: *mut Buffer,
            ) -> u32 {
                if !unsafe { empty(error_buffer) } {
                    return 4;
                }
                let prepared = (|| {
                    unsafe { record(session_slot) }?;
                    let function = unsafe { record(function) }?;
                    let (_, output_layout) =
                        unsafe { owned_layout(function.output_type) }.map_err(managed::invalid)?;
                    let (_, fault_layout) =
                        unsafe { owned_layout(function.fault_type) }.map_err(managed::invalid)?;
                    if out.is_null()
                        || !(out as usize).is_multiple_of(output_layout.align())
                        || fault.is_null()
                        || !(fault as usize).is_multiple_of(fault_layout.align())
                    {
                        return Err(managed::invalid("invalid API root outputs"));
                    }
                    let append = append_cause.ok_or_else(|| {
                        managed::invalid("missing application cleanup cause callback")
                    })?;
                    let future =
                        unsafe { ForeignFuture::new(function, input) }.map_err(managed::invalid)?;
                    Ok((future, append))
                })();
                let (future, append) = match prepared {
                    Ok(value) => value,
                    Err(failure) => return unsafe { error(error_buffer, 4, failure.message) },
                };
                let result = task::run_entry_with_typed_resource_cleanup(
                    task::RuntimeConfig::default(),
                    application::scope_entry(future),
                    |result, resources| async move {
                        let result = cleanup_result(result, resources, append);
                        let raw = unsafe { *session_slot };
                        if raw.is_null() {
                            return result;
                        }
                        let session = unsafe { session(raw) }
                            .map_err(|error| ForeignFault::Runtime(error.message))?;
                        let cleanup = session.clone().scope(session.close()).await;
                        cleanup_result(result, cleanup, append)
                    },
                );
                unsafe { root_result(result, out, fault, error_buffer) }
            }
        }

        #[cfg(feature = "runtime-external")]
        mod external {
            use super::*;
            use crate::ABI_OK;
            use dever_runtime::{component, external as resources, wire};

            #[repr(C)]
            pub struct Name {
                pointer: *const u8,
                len: u64,
            }

            #[repr(C)]
            pub struct Definition {
                key: Name,
                ecosystem: Name,
                entry: Name,
                port: Name,
                adapter: Name,
                schema: Name,
                capabilities: *const Name,
                capability_count: u64,
                operations: *const Name,
                operation_count: u64,
                timeout_ms: u64,
            }

            #[repr(C)]
            pub struct Resource {
                path: Name,
                bytes: *const u8,
                len: u64,
                sha256: Name,
                executable: u8,
            }

            unsafe fn array<'a, T>(
                raw: *const T,
                count: u64,
            ) -> Result<&'a [T], managed::AbiFailure> {
                let count = usize::try_from(count)
                    .map_err(|_| managed::invalid("invalid external array count"))?;
                if count == 0 {
                    return Ok(&[]);
                }
                if raw.is_null()
                    || !raw.is_aligned()
                    || count > isize::MAX as usize / size_of::<T>().max(1)
                {
                    return Err(managed::invalid("invalid external array"));
                }
                Ok(unsafe { std::slice::from_raw_parts(raw, count) })
            }

            unsafe fn name<'a>(raw: &Name) -> Result<&'a str, managed::AbiFailure> {
                std::str::from_utf8(unsafe { array(raw.pointer, raw.len) }?)
                    .map_err(|_| managed::invalid("external name is not UTF-8"))
            }

            unsafe fn names(
                raw: *const Name,
                count: u64,
            ) -> Result<Vec<String>, managed::AbiFailure> {
                unsafe { array(raw, count) }?
                    .iter()
                    .map(|value| unsafe { name(value) }.map(str::to_owned))
                    .collect()
            }

            /// # Safety
            /// Metadata and resource bytes are immutable process-static compiler output.
            /// The descriptor array itself is borrowed only during this call.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_external_prepare(
                digest: *const Name,
                rows: *const Resource,
                count: u64,
                buffer: *mut Buffer,
            ) -> u32 {
                if !unsafe { empty(buffer) } {
                    return ABI_INVALID_INPUT;
                }
                let result = (|| {
                    let digest = unsafe { managed::borrowed::<Name>(digest.cast()) }?;
                    let digest = unsafe { name(digest) }?;
                    let resources = unsafe { array(rows, count) }?
                        .iter()
                        .map(|row| {
                            Ok(resources::Resource {
                                path: unsafe { name(&row.path) }?,
                                bytes: unsafe { array(row.bytes, row.len) }?,
                                sha256: unsafe { name(&row.sha256) }?,
                                executable: match row.executable {
                                    0 => false,
                                    1 => true,
                                    _ => {
                                        return Err(managed::invalid(
                                            "invalid external executable flag",
                                        ));
                                    }
                                },
                            })
                        })
                        .collect::<Result<Vec<_>, managed::AbiFailure>>()?;
                    let root = dever_runtime::config::executable_directory()
                        .map_err(managed::runtime_error)?;
                    resources::prepare(&root, digest, &resources).map_err(managed::runtime_error)
                })();
                match result {
                    Ok(()) => ABI_OK,
                    Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
                }
            }

            /// # Safety
            /// The definition and optional setting Text are borrowed only during creation.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_external_start(
                raw: *const Definition,
                setting: *const c_void,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(buffer) } {
                    return ptr::null_mut();
                }
                let prepared: Result<component::Definition, managed::AbiFailure> = (|| {
                    let row = unsafe { managed::borrowed::<Definition>(raw.cast()) }?;
                    Ok(component::Definition {
                        key: unsafe { name(&row.key) }?.to_owned(),
                        ecosystem: unsafe { name(&row.ecosystem) }?.to_owned(),
                        entry: unsafe { name(&row.entry) }?.to_owned(),
                        port: unsafe { name(&row.port) }?.to_owned(),
                        adapter: unsafe { name(&row.adapter) }?.to_owned(),
                        schema: unsafe { name(&row.schema) }?.to_owned(),
                        capabilities: unsafe { names(row.capabilities, row.capability_count) }?,
                        operations: unsafe { names(row.operations, row.operation_count) }?,
                        setting: if setting.is_null() {
                            None
                        } else {
                            Some(unsafe { managed::borrowed::<String>(setting) }?.clone())
                        },
                        timeout_ms: row.timeout_ms,
                    })
                })(
                );
                let definition = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { error(buffer, failure.status, failure.message) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        component::start(definition)
                            .await
                            .map_err(ForeignFault::from)?;
                        Ok(OpValue::Unit)
                    },
                    Some(OutputSlots::Unit),
                )
            }

            /// # Safety
            /// All Text handles are borrowed until return; the operation owns its inputs.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_external_call(
                key: *const c_void,
                operation_name: *const c_void,
                payload: *const c_void,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(buffer) } {
                    return ptr::null_mut();
                }
                let prepared: Result<_, managed::AbiFailure> = (|| {
                    let key = unsafe { managed::borrowed::<String>(key) }?.clone();
                    let operation = unsafe { managed::borrowed::<String>(operation_name) }?.clone();
                    let payload = unsafe { managed::borrowed::<String>(payload) }?;
                    let mut encoder = wire::Encoder::default();
                    encoder.json(payload).map_err(managed::runtime_error)?;
                    Ok((
                        key,
                        operation,
                        encoder.finish().map_err(managed::runtime_error)?,
                    ))
                })();
                let (key, operation, payload) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { error(buffer, failure.status, failure.message) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        component::call(&key, &operation, &payload)
                            .await
                            .map(OpValue::External)
                            .map_err(ForeignFault::from)
                    },
                    Some(OutputSlots::Handle),
                )
            }

            /// # Safety
            /// Consumes one Reply on every path. Outputs are distinct writable slots.
            /// Success initializes kind (0=result, 1=declared error) and two owned Texts.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_external_reply_take(
                raw: *mut c_void,
                kind: *mut u8,
                identity: *mut *mut c_void,
                payload: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                let reply = unsafe { managed::consumed::<component::Reply>(raw) };
                if !unsafe { empty(buffer) } {
                    return ABI_INVALID_INPUT;
                }
                if kind.is_null()
                    || identity.is_null()
                    || !identity.is_aligned()
                    || payload.is_null()
                    || !payload.is_aligned()
                {
                    return unsafe {
                        error(buffer, ABI_INVALID_INPUT, "invalid external reply output")
                    };
                }
                match reply {
                    Ok(reply) => {
                        let (tag, error_identity, body) = match &*reply {
                            component::Reply::Result(body) => (0, String::new(), body.clone()),
                            component::Reply::Error { identity, payload } => {
                                (1, identity.clone(), payload.clone())
                            }
                        };
                        unsafe {
                            kind.write(tag);
                            identity.write(managed::owned(error_identity));
                            payload.write(managed::owned(body));
                        }
                        ABI_OK
                    }
                    Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
                }
            }

            /// # Safety
            /// Transfers one Reply handle returned by external_call.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_external_reply_release(raw: *mut c_void) {
                unsafe { managed::release::<component::Reply>(raw) };
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn dever_rt_v1_external_shutdown() -> *mut AsyncOp {
                operation_with_output(
                    async {
                        component::shutdown().await.map_err(ForeignFault::from)?;
                        Ok(OpValue::Unit)
                    },
                    Some(OutputSlots::Unit),
                )
            }

            /// # Safety
            /// cause is an initialized nullable Text slot; message is borrowed Text.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_external_cause_append(
                cause: *mut *mut c_void,
                message: *const c_void,
            ) {
                unsafe { managed::append_fault_cause(cause, message) };
            }

            /// # Safety
            /// Static callbacks and typed output/fault slots follow async_root.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_external_root(
                function: *const AsyncFunction,
                input: *const c_void,
                output: *mut c_void,
                fault: *mut c_void,
                append_cause: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
                buffer: *mut Buffer,
            ) -> u32 {
                if !unsafe { empty(buffer) } {
                    return 4;
                }
                let prepared = (|| {
                    let function = unsafe { managed::borrowed::<AsyncFunction>(function.cast()) }
                        .map_err(|failure| failure.message)?;
                    let (_, output_layout) = unsafe { owned_layout(function.output_type) }?;
                    let (_, fault_layout) = unsafe { owned_layout(function.fault_type) }?;
                    if output.is_null()
                        || !(output as usize).is_multiple_of(output_layout.align())
                        || fault.is_null()
                        || !(fault as usize).is_multiple_of(fault_layout.align())
                    {
                        return Err("invalid external root outputs".to_owned());
                    }
                    let append = append_cause.ok_or("missing external cleanup callback")?;
                    let future = unsafe { ForeignFuture::new(function, input) }?;
                    Ok((future, append))
                })();
                let (future, append) = match prepared {
                    Ok(value) => value,
                    Err(message) => return unsafe { error(buffer, 4, message) },
                };
                let result = task::run_entry_with_typed_resource_cleanup(
                    task::RuntimeConfig::default(),
                    future,
                    |result, resources| {
                        std::future::ready(cleanup_result(result, resources, append))
                    },
                );
                unsafe { root_result(result, output, fault, buffer) }
            }
        }

        #[cfg(any(feature = "runtime-sqlite", feature = "runtime-postgres"))]
        mod database {
            use super::*;
            use dever_runtime::{database as db, orm};

            #[repr(C)]
            #[derive(Clone, Copy)]
            pub struct Name {
                pointer: *const u8,
                len: u64,
            }
            #[repr(C)]
            pub struct ErrorDescriptor {
                fault_type: *const OwnedType,
                pack: Option<unsafe extern "C" fn(u32, *const c_void, *mut c_void)>,
                append_cause: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
            }
            #[repr(C)]
            pub struct Sql {
                sqlite: Name,
                postgres: Name,
            }
            #[repr(C)]
            pub struct Value {
                kind: u32,
                precision: u32,
                scale: u32,
                integer: i64,
                floating: f64,
                decimal: crate::AbiDecimal,
                handle: *const c_void,
            }
            #[repr(C)]
            pub struct Binding {
                explicit: *const Name,
                root: Name,
                tenant: u8,
            }
            #[repr(C)]
            pub struct TransactionBindings {
                bindings: *const Binding,
                count: u64,
            }
            #[repr(C)]
            pub struct Field {
                name: Name,
                ty: Name,
                nullable: u8,
                generated: u8,
                default: *const Name,
                rename: *const Name,
            }
            #[repr(C)]
            pub struct Index {
                name: Name,
                fields: *const Name,
                count: u64,
                unique: u8,
            }
            #[repr(C)]
            pub struct Migration {
                name: Name,
                revision: Name,
                drops: *const Name,
                count: u64,
            }
            #[repr(C)]
            pub struct Seed {
                sql: Sql,
                parameters: *const Value,
                count: u64,
            }
            #[repr(C)]
            pub struct DataMigration {
                name: Name,
                phase: u32,
                statement: Seed,
            }
            #[repr(C)]
            pub struct PostgresColumn {
                name: Name,
                definition: Name,
                sql_type: Name,
                default: *const Name,
            }
            #[repr(C)]
            pub struct PostgresConstraint {
                name: Name,
                definition: Name,
                foreign_key: u8,
            }
            #[repr(C)]
            pub struct Model {
                model: Name,
                table: Name,
                revision: Name,
                seed_revision: Name,
                fields: *const Field,
                field_count: u64,
                indexes: *const Index,
                index_count: u64,
                migrations: *const Migration,
                migration_count: u64,
                create_table: Sql,
                temporary_table: Name,
                create_indexes: *const Sql,
                create_index_count: u64,
                postgres_columns: *const PostgresColumn,
                postgres_column_count: u64,
                postgres_constraints: *const PostgresConstraint,
                postgres_constraint_count: u64,
                seeds: *const Seed,
                seed_count: u64,
                data_migrations: *const DataMigration,
                data_migration_count: u64,
            }
            #[repr(C)]
            pub struct RowDecoder {
                row_type: *const OwnedType,
                decode: Option<
                    unsafe extern "C" fn(
                        *const c_void,
                        *mut c_void,
                        *const ErrorDescriptor,
                        *mut c_void,
                        *mut Buffer,
                    ) -> u32,
                >,
            }

            enum Failure {
                Abi(String),
                Database(orm::Error),
            }
            impl From<managed::AbiFailure> for Failure {
                fn from(value: managed::AbiFailure) -> Self {
                    Self::Abi(value.message)
                }
            }
            impl From<orm::Error> for Failure {
                fn from(value: orm::Error) -> Self {
                    Self::Database(value)
                }
            }
            fn invalid(message: impl Into<String>) -> Failure {
                Failure::Abi(message.into())
            }

            unsafe fn record<'a, T>(pointer: *const T) -> Result<&'a T, Failure> {
                if pointer.is_null() || !pointer.is_aligned() {
                    return Err(invalid("invalid database ABI record"));
                }
                Ok(unsafe { &*pointer })
            }
            unsafe fn array<'a, T>(pointer: *const T, count: u64) -> Result<&'a [T], Failure> {
                let count =
                    usize::try_from(count).map_err(|_| invalid("invalid database ABI count"))?;
                if count == 0 {
                    return Ok(&[]);
                }
                if pointer.is_null()
                    || !pointer.is_aligned()
                    || count > isize::MAX as usize / size_of::<T>().max(1)
                {
                    return Err(invalid("invalid database ABI array"));
                }
                Ok(unsafe { std::slice::from_raw_parts(pointer, count) })
            }
            unsafe fn name<'a>(value: &Name) -> Result<&'a str, Failure> {
                std::str::from_utf8(unsafe { array(value.pointer, value.len) }?)
                    .map_err(|_| invalid("database ABI name is not UTF-8"))
            }
            unsafe fn optional_name(value: *const Name) -> Result<Option<String>, Failure> {
                if value.is_null() {
                    Ok(None)
                } else {
                    Ok(Some(unsafe { name(record(value)?) }?.to_owned()))
                }
            }
            fn flag(value: u8) -> Result<bool, Failure> {
                match value {
                    0 => Ok(false),
                    1 => Ok(true),
                    _ => Err(invalid("invalid database ABI boolean")),
                }
            }
            unsafe fn cloned<T: Clone>(handle: *const c_void) -> Result<T, Failure> {
                Ok(unsafe { managed::borrowed::<T>(handle) }?.clone())
            }
            unsafe fn shared<T>(handle: *const c_void) -> Result<Arc<T>, Failure> {
                unsafe { managed::borrowed::<T>(handle) }?;
                // SAFETY: a validated borrowed Arc owner is retained for this operation.
                unsafe {
                    Arc::increment_strong_count(handle.cast::<T>());
                }
                Ok(unsafe { Arc::from_raw(handle.cast::<T>()) })
            }
            unsafe fn sql(value: &Sql) -> Result<db::Sql, Failure> {
                // SQL bytes are immutable process-static compiler literals, as
                // required by both existing driver APIs. No caller data is leaked.
                Ok(db::Sql {
                    sqlite: unsafe { name(&value.sqlite) }?,
                    postgres: unsafe { name(&value.postgres) }?,
                })
            }
            unsafe fn schema_sql(value: &Sql) -> Result<db::SchemaSql, Failure> {
                Ok(db::SchemaSql {
                    sqlite: unsafe { name(&value.sqlite) }?.into(),
                    postgres: unsafe { name(&value.postgres) }?.into(),
                })
            }
            unsafe fn parameters(
                values: *const Value,
                count: u64,
            ) -> Result<Vec<orm::Value>, Failure> {
                unsafe { array(values, count) }?
                    .iter()
                    .map(|value| {
                        Ok(match value.kind {
                            0 => orm::Value::Null,
                            1 => orm::Value::Bool(match value.integer {
                                0 => false,
                                1 => true,
                                _ => return Err(invalid("invalid database Bool bind")),
                            }),
                            2 => orm::Value::Int(value.integer),
                            3 if value.floating.is_finite() => orm::Value::Float(value.floating),
                            3 => {
                                return Err(orm::Error::invalid_data(
                                    "database Float must be finite",
                                )
                                .into());
                            }
                            4 => orm::decimal_value(
                                value.decimal.decode().map_err(invalid)?,
                                u8::try_from(value.precision)
                                    .map_err(|_| invalid("invalid Decimal precision"))?,
                                u8::try_from(value.scale)
                                    .map_err(|_| invalid("invalid Decimal scale"))?,
                            )?,
                            5 => orm::Value::Text(unsafe { cloned(value.handle) }?),
                            6 => orm::Value::Bytes(
                                unsafe { managed::borrowed::<Bytes>(value.handle) }?
                                    .values()
                                    .to_vec(),
                            ),
                            7 => orm::Value::Uuid(unsafe { cloned(value.handle) }?),
                            _ => return Err(invalid("invalid database bind kind")),
                        })
                    })
                    .collect()
            }
            unsafe fn model(value: *const Model) -> Result<db::Model, Failure> {
                let value = unsafe { record(value) }?;
                let fields = unsafe { array(value.fields, value.field_count) }?
                    .iter()
                    .map(|field| {
                        Ok(db::Field {
                            name: unsafe { name(&field.name) }?.into(),
                            ty: unsafe { name(&field.ty) }?.into(),
                            nullable: flag(field.nullable)?,
                            generated: flag(field.generated)?,
                            default: unsafe { optional_name(field.default) }?,
                            rename_from: unsafe { optional_name(field.rename) }?,
                        })
                    })
                    .collect::<Result<_, Failure>>()?;
                let indexes = unsafe { array(value.indexes, value.index_count) }?
                    .iter()
                    .map(|index| {
                        Ok(db::Index {
                            name: unsafe { name(&index.name) }?.into(),
                            unique: flag(index.unique)?,
                            fields: unsafe { array(index.fields, index.count) }?
                                .iter()
                                .map(|field| unsafe { name(field) }.map(str::to_owned))
                                .collect::<Result<_, _>>()?,
                        })
                    })
                    .collect::<Result<_, Failure>>()?;
                let migrations = unsafe { array(value.migrations, value.migration_count) }?
                    .iter()
                    .map(|migration| {
                        Ok(db::Migration {
                            name: unsafe { name(&migration.name) }?.into(),
                            revision: unsafe { name(&migration.revision) }?.into(),
                            drops: unsafe { array(migration.drops, migration.count) }?
                                .iter()
                                .map(|field| unsafe { name(field) }.map(str::to_owned))
                                .collect::<Result<_, _>>()?,
                        })
                    })
                    .collect::<Result<_, Failure>>()?;
                let postgres_columns =
                    unsafe { array(value.postgres_columns, value.postgres_column_count) }?
                        .iter()
                        .map(|column| {
                            Ok(db::PostgresColumn {
                                name: unsafe { name(&column.name) }?.into(),
                                definition: unsafe { name(&column.definition) }?.into(),
                                sql_type: unsafe { name(&column.sql_type) }?.into(),
                                default: unsafe { optional_name(column.default) }?,
                            })
                        })
                        .collect::<Result<_, Failure>>()?;
                let postgres_constraints = unsafe {
                    constraints(value.postgres_constraints, value.postgres_constraint_count)
                }?;
                let seeds = unsafe { array(value.seeds, value.seed_count) }?
                    .iter()
                    .map(|seed| {
                        Ok(db::Seed {
                            sql: unsafe { sql(&seed.sql) }?,
                            parameters: unsafe { parameters(seed.parameters, seed.count) }?,
                        })
                    })
                    .collect::<Result<_, Failure>>()?;
                let data_migrations =
                    unsafe { array(value.data_migrations, value.data_migration_count) }?
                        .iter()
                        .map(|migration| {
                            Ok(db::DataMigration {
                                name: unsafe { name(&migration.name) }?.into(),
                                phase: match migration.phase {
                                    0 => db::MigrationPhase::Before,
                                    1 => db::MigrationPhase::After,
                                    _ => return Err(invalid("invalid database migration phase")),
                                },
                                sql: unsafe { sql(&migration.statement.sql) }?,
                                parameters: unsafe {
                                    parameters(
                                        migration.statement.parameters,
                                        migration.statement.count,
                                    )
                                }?,
                            })
                        })
                        .collect::<Result<_, Failure>>()?;
                Ok(db::Model {
                    schema: db::Schema {
                        model: unsafe { name(&value.model) }?.into(),
                        table: unsafe { name(&value.table) }?.into(),
                        revision: unsafe { name(&value.revision) }?.into(),
                        seed_revision: unsafe { name(&value.seed_revision) }?.into(),
                        fields,
                        indexes,
                        migrations,
                    },
                    create_table: unsafe { schema_sql(&value.create_table) }?,
                    create_temporary_table: unsafe { name(&value.temporary_table) }?.into(),
                    create_indexes: unsafe {
                        array(value.create_indexes, value.create_index_count)
                    }?
                    .iter()
                    .map(|sql| unsafe { schema_sql(sql) })
                    .collect::<Result<_, _>>()?,
                    postgres_columns,
                    postgres_constraints,
                    seeds,
                    data_migrations,
                })
            }
            unsafe fn constraints(
                pointer: *const PostgresConstraint,
                count: u64,
            ) -> Result<Vec<db::PostgresConstraint>, Failure> {
                unsafe { array(pointer, count) }?
                    .iter()
                    .map(|constraint| {
                        Ok(db::PostgresConstraint {
                            name: unsafe { name(&constraint.name) }?.into(),
                            definition: unsafe { name(&constraint.definition) }?.into(),
                            foreign_key: flag(constraint.foreign_key)?,
                        })
                    })
                    .collect()
            }

            #[derive(Clone, Copy)]
            struct Errors {
                descriptor: &'static ErrorDescriptor,
                ty: &'static OwnedType,
                align: usize,
            }
            // Static callbacks and immutable descriptors are compiler output.
            // new validates the complete fault's transferable owner descriptor.
            unsafe impl Send for Errors {}
            unsafe impl Sync for Errors {}
            impl Errors {
                unsafe fn new(pointer: *const ErrorDescriptor) -> Result<Self, Failure> {
                    let descriptor = unsafe { record(pointer) }?;
                    let (ty, layout) =
                        unsafe { owned_layout(descriptor.fault_type) }.map_err(invalid)?;
                    if descriptor.pack.is_none()
                        || descriptor.append_cause.is_none()
                        || ty.transferable == 0
                    {
                        return Err(invalid("invalid database fault descriptor"));
                    }
                    Ok(Self {
                        descriptor,
                        ty,
                        align: layout.align(),
                    })
                }
                fn fault(self, error: orm::Error) -> ForeignFault {
                    let mut value = match unsafe { Owned::new(self.ty) } {
                        Ok(value) => value,
                        Err(message) => return ForeignFault::Runtime(message),
                    };
                    let message = Arc::new(error.to_string());
                    unsafe {
                        (self.descriptor.pack.expect("validated callback"))(
                            kind(error.kind()),
                            Arc::as_ptr(&message).cast(),
                            value.pointer(),
                        )
                    };
                    value.initialized = true;
                    ForeignFault::Typed(value)
                }
                fn failure(self, failure: Failure) -> ForeignFault {
                    match failure {
                        Failure::Database(error) => self.fault(error),
                        Failure::Abi(message) => ForeignFault::Runtime(message),
                    }
                }
                fn augment(self, primary: ForeignFault, cleanup: orm::Error) -> ForeignFault {
                    match primary {
                        ForeignFault::Typed(value) => {
                            let message = Arc::new(cleanup.to_string());
                            unsafe {
                                (self.descriptor.append_cause.expect("validated callback"))(
                                    value.pointer(),
                                    Arc::as_ptr(&message).cast(),
                                )
                            };
                            ForeignFault::Typed(value)
                        }
                        ForeignFault::Runtime(message) => {
                            ForeignFault::Runtime(format!("{message}; caused by: {cleanup}"))
                        }
                    }
                }
                fn slots(self, align: usize, optional: bool) -> OutputSlots {
                    OutputSlots::Database {
                        align,
                        optional,
                        fault_align: self.align,
                    }
                }
            }
            fn kind(value: orm::ErrorKind) -> u32 {
                match value {
                    orm::ErrorKind::Pool => 0,
                    orm::ErrorKind::PoolExhausted => 1,
                    orm::ErrorKind::Connection => 2,
                    orm::ErrorKind::Timeout => 3,
                    orm::ErrorKind::Cancelled => 4,
                    orm::ErrorKind::Database => 5,
                    orm::ErrorKind::Constraint => 6,
                    orm::ErrorKind::NotFound => 7,
                    orm::ErrorKind::InvalidData => 8,
                    orm::ErrorKind::Migration => 9,
                }
            }
            fn error_kind(value: u32) -> Result<orm::ErrorKind, Failure> {
                match value {
                    0 => Ok(orm::ErrorKind::Pool),
                    1 => Ok(orm::ErrorKind::PoolExhausted),
                    2 => Ok(orm::ErrorKind::Connection),
                    3 => Ok(orm::ErrorKind::Timeout),
                    4 => Ok(orm::ErrorKind::Cancelled),
                    5 => Ok(orm::ErrorKind::Database),
                    6 => Ok(orm::ErrorKind::Constraint),
                    7 => Ok(orm::ErrorKind::NotFound),
                    8 => Ok(orm::ErrorKind::InvalidData),
                    9 => Ok(orm::ErrorKind::Migration),
                    _ => Err(invalid("invalid database error kind")),
                }
            }
            unsafe fn checked<T>(
                out: *mut T,
                errors: *const ErrorDescriptor,
                fault: *mut c_void,
                buffer: *mut Buffer,
            ) -> Result<Errors, u32> {
                unsafe { managed::checked_output(out, buffer) }?;
                let result = unsafe { Errors::new(errors) }.and_then(|errors| {
                    if fault.is_null() || !(fault as usize).is_multiple_of(errors.align) {
                        Err(invalid("invalid database fault output"))
                    } else {
                        Ok(errors)
                    }
                });
                result.map_err(|failure| unsafe { abi_failure(failure, buffer) })
            }
            unsafe fn abi_failure(failure: Failure, buffer: *mut Buffer) -> u32 {
                let message = match failure {
                    Failure::Abi(message) => message,
                    Failure::Database(error) => error.to_string(),
                };
                unsafe { error(buffer, 2, message) }
            }
            unsafe fn finish<T>(
                result: Result<T, Failure>,
                out: *mut T,
                errors: Errors,
                fault: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                match result {
                    Ok(value) => {
                        unsafe { out.write(value) };
                        0
                    }
                    Err(Failure::Database(database_error)) => match errors.fault(database_error) {
                        ForeignFault::Typed(mut value) => {
                            unsafe { value.move_to(fault) };
                            1
                        }
                        ForeignFault::Runtime(message) => unsafe { error(buffer, 2, message) },
                    },
                    Err(failure) => unsafe { abi_failure(failure, buffer) },
                }
            }
            unsafe fn regular<T>(
                out: *mut T,
                errors: *const ErrorDescriptor,
                fault: *mut c_void,
                buffer: *mut Buffer,
                apply: impl FnOnce() -> Result<T, Failure>,
            ) -> u32 {
                let errors = match unsafe { checked(out, errors, fault, buffer) } {
                    Ok(errors) => errors,
                    Err(status) => return status,
                };
                unsafe { finish(apply(), out, errors, fault, buffer) }
            }

            type Transaction = Arc<dever_runtime::database::TransactionState>;

            fn completed<T>(
                mut transaction: Option<db::TransactionLease<'_>>,
                result: Result<T, orm::Error>,
            ) -> Result<T, Failure> {
                if let Some(transaction) = &mut transaction {
                    transaction.complete();
                }
                result.map_err(Failure::Database)
            }

            pub(super) enum Output {
                Unit,
                Database(db::Database),
                #[cfg(feature = "runtime-api")]
                Text(String),
                Int(i64),
                Transaction(Transaction),
                Rows(Vec<orm::Row>),
                Stream(Stream),
                Row(Option<Owned>),
            }
            impl Output {
                pub(super) unsafe fn write(self, out: *mut c_void, present: *mut u8) {
                    match self {
                        Self::Unit => {}
                        Self::Database(value) => unsafe {
                            out.cast::<*mut c_void>().write(managed::owned(value))
                        },
                        #[cfg(feature = "runtime-api")]
                        Self::Text(value) => unsafe {
                            out.cast::<*mut c_void>().write(managed::owned(value))
                        },
                        Self::Int(value) => unsafe { out.cast::<i64>().write(value) },
                        Self::Transaction(value) => unsafe {
                            out.cast::<*mut c_void>().write(managed::owned(value))
                        },
                        Self::Rows(value) => unsafe {
                            out.cast::<*mut c_void>().write(managed::owned(value))
                        },
                        Self::Stream(value) => unsafe {
                            out.cast::<*mut c_void>().write(managed::owned(value))
                        },
                        Self::Row(value) => {
                            unsafe { present.write(u8::from(value.is_some())) };
                            if let Some(mut value) = value {
                                unsafe { value.move_to(out) };
                            }
                        }
                    }
                }
            }
            unsafe fn operation<P, F>(
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
                align: usize,
                optional: bool,
                prepare: impl FnOnce() -> Result<P, Failure>,
                run: impl FnOnce(P) -> F + 'static,
            ) -> *mut AsyncOp
            where
                P: 'static,
                F: Future<Output = Result<Output, Failure>> + 'static,
            {
                if !unsafe { empty(buffer) } {
                    return ptr::null_mut();
                }
                let errors = match unsafe { Errors::new(errors) } {
                    Ok(errors) => errors,
                    Err(failure) => {
                        unsafe { abi_failure(failure, buffer) };
                        return ptr::null_mut();
                    }
                };
                let prepared = prepare();
                if let Err(Failure::Abi(message)) = prepared {
                    unsafe { error(buffer, 2, message) };
                    return ptr::null_mut();
                }
                operation_with_output(
                    async move {
                        let prepared = prepared.map_err(|failure| errors.failure(failure))?;
                        run(prepared)
                            .await
                            .map(OpValue::Database)
                            .map_err(|failure| errors.failure(failure))
                    },
                    Some(errors.slots(align, optional)),
                )
            }

            struct Query {
                database: db::Database,
                transaction: Option<Transaction>,
                sql: db::Sql,
                parameters: Vec<orm::Value>,
            }
            impl Query {
                unsafe fn new(
                    database: *const c_void,
                    transaction: *const c_void,
                    sql_pointer: *const Sql,
                    values: *const Value,
                    count: u64,
                ) -> Result<Self, Failure> {
                    Ok(Self {
                        database: unsafe { cloned(database) }?,
                        transaction: if transaction.is_null() {
                            None
                        } else {
                            Some(unsafe { cloned(transaction) }?)
                        },
                        sql: unsafe { sql(record(sql_pointer)?) }?,
                        parameters: unsafe { parameters(values, count) }?,
                    })
                }
            }

            macro_rules! owner {
                ($retain:ident, $release:ident, $ty:ty) => {
                    /// # Safety
                    /// The handle is a live borrowed owner of the named database type, or null.
                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn $retain(handle: *const c_void) -> *mut c_void {
                        unsafe { managed::retain::<$ty>(handle) }
                    }
                    /// # Safety
                    /// Transfers one matching database owner, or null.
                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn $release(handle: *mut c_void) {
                        unsafe { managed::release::<$ty>(handle) }
                    }
                };
            }
            owner!(dever_rt_v1_db_retain, dever_rt_v1_db_release, db::Database);
            owner!(
                dever_rt_v1_db_transaction_retain,
                dever_rt_v1_db_transaction_release,
                Transaction
            );
            owner!(
                dever_rt_v1_db_stream_retain,
                dever_rt_v1_db_stream_release,
                Stream
            );
            owner!(
                dever_rt_v1_db_related_retain,
                dever_rt_v1_db_related_release,
                Element
            );

            #[cfg(feature = "runtime-api")]
            /// # Safety
            /// Borrows the registry and static DbError mapping; output is Unit.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_job_initialize(
                jobs: *const c_void,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                unsafe {
                    operation(
                        errors,
                        buffer,
                        0,
                        false,
                        || super::api_application::job_registry(jobs).map_err(Failure::from),
                        |jobs| async move {
                            jobs.application
                                .clone()
                                .scope(jobs.resources.initialize())
                                .await?;
                            Ok(Output::Unit)
                        },
                    )
                }
            }

            #[cfg(feature = "runtime-api")]
            /// # Safety
            /// All handles are borrowed; nullable transaction is the caller's physical owner.
            /// The operation retains owners and validates output before queue I/O.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_job_enqueue(
                jobs: *const c_void,
                index: u64,
                database: *const c_void,
                transaction: *const c_void,
                payload: *const c_void,
                key: *const c_void,
                run_at: i64,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                unsafe {
                    operation(
                        errors,
                        buffer,
                        align_of::<*mut c_void>(),
                        false,
                        || {
                            let jobs = super::api_application::job_registry(jobs)?;
                            let index =
                                usize::try_from(index).map_err(|_| invalid("invalid Job index"))?;
                            let spec = jobs
                                .resources
                                .bindings
                                .get(index)
                                .ok_or_else(|| invalid("unknown compiled Job index"))?
                                .spec
                                .clone();
                            let database = cloned::<db::Database>(database)?;
                            let transaction = if transaction.is_null() {
                                None
                            } else {
                                Some(cloned::<Transaction>(transaction)?)
                            };
                            let mut encoder = dever_runtime::wire::Encoder::default();
                            encoder
                                .json(managed::borrowed::<String>(payload)?)
                                .map_err(invalid)?;
                            let payload = encoder.finish().map_err(invalid)?;
                            let key = cloned::<String>(key)?;
                            Ok((jobs, spec, database, transaction, payload, key))
                        },
                        move |(jobs, spec, database, transaction, payload, key)| async move {
                            jobs.application
                                .clone()
                                .scope(async move {
                                    let mut lease =
                                        db::TransactionState::borrow(transaction.as_deref())
                                            .await?;
                                    let result = dever_runtime::job::enqueue(
                                        database,
                                        lease.as_deref(),
                                        &spec,
                                        payload,
                                        &key,
                                        run_at,
                                    )
                                    .await;
                                    if let Some(lease) = &mut lease {
                                        lease.complete();
                                    }
                                    result.map(|id| Output::Text(id.0)).map_err(Failure::from)
                                })
                                .await
                        },
                    )
                }
            }

            /// # Safety
            /// Bindings are borrowed immutable rows; out is an uninitialized owner slot.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_session_new(
                bindings: *const Binding,
                count: u64,
                transactions: *const TransactionBindings,
                transaction_count: u64,
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result = (|| {
                    let binding = |value: &Binding| -> Result<_, Failure> {
                        Ok((
                            if value.explicit.is_null() {
                                None
                            } else {
                                Some(unsafe { name(record(value.explicit)?) }?)
                            },
                            unsafe { name(&value.root) }?,
                            flag(value.tenant)?,
                        ))
                    };
                    let bindings = unsafe { array(bindings, count) }?
                        .iter()
                        .map(binding)
                        .collect::<Result<Vec<_>, _>>()?;
                    let transactions = unsafe { array(transactions, transaction_count) }?
                        .iter()
                        .map(|transaction| {
                            unsafe { array(transaction.bindings, transaction.count) }?
                                .iter()
                                .map(binding)
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .collect::<Result<Vec<_>, Failure>>()?;
                    let transactions = transactions.iter().map(Vec::as_slice).collect::<Vec<_>>();
                    Ok(db::Session::load(
                        dever_runtime::config::RuntimeProfile {
                            sqlite: cfg!(feature = "runtime-sqlite"),
                            postgres: cfg!(feature = "runtime-postgres"),
                        },
                        &bindings,
                        &transactions,
                    ))
                })();
                match result {
                    Ok(Ok(session)) => {
                        unsafe { out.write(managed::owned(session)) };
                        0
                    }
                    Ok(Err(message)) => unsafe { error(buffer, 1, message) },
                    Err(failure) => unsafe { abi_failure(failure, buffer) },
                }
            }
            /// # Safety
            /// Transfers one entry session after the application root has closed it.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_session_release(session: *mut c_void) {
                unsafe { managed::release::<db::Session>(session) }
            }

            /// # Safety
            /// Same callback contract as async_root. session_slot is the module's live
            /// pointer slot, published by the root and kept through cleanup.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_application_root(
                function: *const AsyncFunction,
                input: *const c_void,
                out: *mut c_void,
                fault: *mut c_void,
                session_slot: *const *mut c_void,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> u32 {
                if !unsafe { empty(buffer) } {
                    return 4;
                }
                let prepared = (|| {
                    unsafe { record(session_slot) }?;
                    let errors = unsafe { Errors::new(errors) }?;
                    let function = unsafe { record(function) }?;
                    let (_, layout) =
                        unsafe { owned_layout(function.output_type) }.map_err(invalid)?;
                    if out.is_null()
                        || !(out as usize).is_multiple_of(layout.align())
                        || fault.is_null()
                        || !(fault as usize).is_multiple_of(errors.align)
                        || !std::ptr::eq(function.fault_type, errors.ty)
                    {
                        return Err(invalid("invalid database root output descriptor"));
                    }
                    let future = unsafe { ForeignFuture::new(function, input) }.map_err(invalid)?;
                    Ok((future, errors))
                })();
                let (future, errors) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { abi_failure(failure, buffer) };
                        return 4;
                    }
                };
                let result = task::run_entry_with_typed_resource_cleanup(
                    task::RuntimeConfig::default(),
                    future,
                    |result, resources| async move {
                        let result = cleanup_result(
                            result,
                            resources,
                            errors.descriptor.append_cause.expect("validated callback"),
                        );
                        let handle = unsafe { *session_slot };
                        if handle.is_null() {
                            return result;
                        }
                        let session = unsafe { managed::borrowed::<db::Session>(handle) }
                            .map_err(|failure| ForeignFault::Runtime(failure.message))?;
                        match (result, session.close().await) {
                            (result, Ok(())) => result,
                            (Ok(_), Err(cleanup)) => Err(errors.fault(cleanup)),
                            (Err(primary), Err(cleanup)) => Err(errors.augment(primary, cleanup)),
                        }
                    },
                );
                unsafe { root_result(result, out, fault, buffer) }
            }
            /// # Safety
            /// cause is an initialized nullable owned Text slot; message is borrowed Text.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_cause_append(
                cause: *mut *mut c_void,
                message: *const c_void,
            ) {
                unsafe { managed::append_fault_cause(cause, message) };
            }
            /// # Safety
            /// All inputs are borrowed matching owners and static descriptors.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_prepare(
                session: *const c_void,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                unsafe {
                    operation(
                        errors,
                        buffer,
                        0,
                        false,
                        || shared::<db::Session>(session),
                        |session| async move {
                            session.prepare().await?;
                            Ok(Output::Unit)
                        },
                    )
                }
            }
            /// # Safety
            /// Borrows this entry's Session and a static Model binding/error descriptor.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_resolve_scoped(
                session: *const c_void,
                binding: *const Binding,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                unsafe {
                    operation(
                        errors,
                        buffer,
                        align_of::<*mut c_void>(),
                        false,
                        || {
                            let session = shared::<db::Session>(session)?;
                            let binding = record(binding)?;
                            let explicit = optional_name(binding.explicit)?;
                            let root = name(&binding.root)?.to_owned();
                            let tenant = flag(binding.tenant)?;
                            Ok((session, explicit, root, tenant))
                        },
                        |(session, explicit, root, tenant)| async move {
                            #[cfg(feature = "runtime-api")]
                            if tenant
                                && dever_runtime::config::current_settings().tenant().is_some()
                            {
                                let database = session.database(explicit.as_deref(), &root)?;
                                return Ok(Output::Database(
                                    dever_runtime::tenant::database(database.name()).await?,
                                ));
                            }
                            #[cfg(not(feature = "runtime-api"))]
                            let _ = tenant;
                            Ok(Output::Database(
                                session.database(explicit.as_deref(), &root)?,
                            ))
                        },
                    )
                }
            }
            /// # Safety
            /// The session/binding are borrowed; out and fault are matching writable slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_select(
                session: *const c_void,
                binding: *const Binding,
                out: *mut *mut c_void,
                errors: *const ErrorDescriptor,
                fault: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                unsafe {
                    regular(out, errors, fault, buffer, || {
                        let session = managed::borrowed::<db::Session>(session)?;
                        let binding = record(binding)?;
                        let explicit = if binding.explicit.is_null() {
                            None
                        } else {
                            Some(name(record(binding.explicit)?)?)
                        };
                        flag(binding.tenant)?;
                        Ok(managed::owned(
                            session.database(explicit, name(&binding.root)?)?,
                        ))
                    })
                }
            }
            /// # Safety
            /// Database is borrowed and out is a writable Int slot.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_max_page_size(
                database: *const c_void,
                out: *mut i64,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result =
                    unsafe { managed::borrowed::<db::Database>(database) }.and_then(|database| {
                        i64::try_from(database.max_page_size())
                            .map_err(|_| managed::invalid("database page maximum exceeds Int"))
                    });
                unsafe { managed::complete(out, buffer, result) }
            }
            /// # Safety
            /// Arrays contain matching borrowed database/model/error rows. SQL and
            /// error descriptors are static; all other metadata is copied now.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_migrate_models(
                databases: *const *const c_void,
                models: *const Model,
                errors: *const *const ErrorDescriptor,
                count: u64,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(buffer) } {
                    return ptr::null_mut();
                }
                let prepared = (|| {
                    let databases = unsafe { array(databases, count) }?;
                    let models = unsafe { array(models, count) }?;
                    let errors = unsafe { array(errors, count) }?;
                    let first = unsafe {
                        Errors::new(
                            *errors
                                .first()
                                .ok_or_else(|| invalid("missing database Models"))?,
                        )
                    }?;
                    let mut entries = Vec::with_capacity(databases.len());
                    for ((database, model_row), errors) in databases.iter().zip(models).zip(errors)
                    {
                        let errors = unsafe { Errors::new(*errors) }?;
                        if !std::ptr::eq(first.ty, errors.ty) {
                            return Err(invalid("mismatched database migration fault types"));
                        }
                        let database = unsafe { cloned::<db::Database>(*database) }?;
                        let model = match unsafe { model(model_row) } {
                            Ok(model) => model,
                            Err(Failure::Database(error)) => {
                                return Ok((first, Err((errors, error))));
                            }
                            Err(failure) => return Err(failure),
                        };
                        entries.push(migration_entry(database, model, errors));
                    }
                    entries.sort_by(|left, right| left.0.name().cmp(right.0.name()));
                    Ok((first, Ok(entries)))
                })();
                let (errors, entries) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { abi_failure(failure, buffer) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        let entries = entries.map_err(|(errors, error)| errors.fault(error))?;
                        migrate_entries(entries).await?;
                        Ok(OpValue::Database(Output::Unit))
                    },
                    Some(errors.slots(0, false)),
                )
            }

            type MigrationEntry = (
                db::Database,
                db::Model,
                (String, String, Vec<db::PostgresConstraint>),
                Errors,
            );
            fn migration_entry(
                database: db::Database,
                model: db::Model,
                errors: Errors,
            ) -> MigrationEntry {
                let keys = model
                    .postgres_constraints
                    .iter()
                    .filter(|constraint| constraint.foreign_key)
                    .map(|constraint| db::PostgresConstraint {
                        name: constraint.name.clone(),
                        definition: constraint.definition.clone(),
                        foreign_key: true,
                    })
                    .collect();
                let keys = (model.schema.model.clone(), model.schema.table.clone(), keys);
                (database, model, keys, errors)
            }
            async fn migrate_entries(mut entries: Vec<MigrationEntry>) -> Result<(), ForeignFault> {
                entries.sort_by(|left, right| left.0.name().cmp(right.0.name()));
                let mut foreign_keys = Vec::with_capacity(entries.len());
                for (database, model, keys, errors) in entries {
                    database
                        .migrate(model)
                        .await
                        .map_err(|error| errors.fault(error))?;
                    foreign_keys.push((database, keys, errors));
                }
                for (database, (model, table, keys), errors) in foreign_keys {
                    if !keys.is_empty() {
                        database
                            .migrate_foreign_keys(model, table, keys)
                            .await
                            .map_err(|error| errors.fault(error))?;
                    }
                }
                Ok(())
            }

            #[cfg(feature = "runtime-api")]
            /// # Safety
            /// Session is borrowed. Parallel arrays are immutable compiler metadata.
            /// Zero tenant_id migrates platform Models; positive ids are explicit migration.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_migrate_models(
                session: *const c_void,
                bindings: *const Binding,
                models: *const Model,
                errors: *const *const ErrorDescriptor,
                count: u64,
                tenant_id: i64,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                unsafe {
                    dever_rt_v1_api_migrate_application(
                        session,
                        bindings,
                        models,
                        errors,
                        count,
                        ptr::null(),
                        tenant_id,
                        buffer,
                    )
                }
            }

            #[cfg(feature = "runtime-api")]
            /// # Safety
            /// Same immutable metadata contract as api_migrate_models. Empty Model arrays
            /// require a fallback fault descriptor for applications containing only Jobs.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_migrate_application(
                session: *const c_void,
                bindings: *const Binding,
                models: *const Model,
                errors: *const *const ErrorDescriptor,
                count: u64,
                fallback_errors: *const ErrorDescriptor,
                tenant_id: i64,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(buffer) } {
                    return ptr::null_mut();
                }
                let prepared = (|| {
                    let session = unsafe {
                        managed::borrowed::<Arc<dever_runtime::application::Session>>(session)
                    }?
                    .clone();
                    let bindings = unsafe { array(bindings, count) }?;
                    let models = unsafe { array(models, count) }?;
                    let error_rows = unsafe { array(errors, count) }?;
                    let first = unsafe {
                        Errors::new(
                            error_rows
                                .first()
                                .copied()
                                .or_else(|| (!fallback_errors.is_null()).then_some(fallback_errors))
                                .ok_or_else(|| {
                                    invalid("missing database migration fault mapping")
                                })?,
                        )
                    }?;
                    let mut definitions = Vec::with_capacity(models.len());
                    for ((binding, model_row), errors) in
                        bindings.iter().zip(models).zip(error_rows)
                    {
                        let errors = unsafe { Errors::new(*errors) }?;
                        if !std::ptr::eq(first.ty, errors.ty) {
                            return Err(invalid("mismatched database migration fault types"));
                        }
                        let explicit = unsafe { optional_name(binding.explicit) }?;
                        let root = unsafe { name(&binding.root) }?.to_owned();
                        let tenant = flag(binding.tenant)?;
                        let model = match unsafe { model(model_row) } {
                            Err(failure @ Failure::Abi(_)) => return Err(failure),
                            result => result,
                        };
                        definitions.push((explicit, root, tenant, model, errors));
                    }
                    Ok::<_, Failure>((session, definitions, first))
                })();
                let (session, definitions, errors) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { abi_failure(failure, buffer) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        session
                            .clone()
                            .scope(async move {
                                let tenant_enabled = session.settings().tenant().is_some();
                                if tenant_id < 0 || (tenant_id > 0 && !tenant_enabled) {
                                    return Err(errors.fault(orm::Error::migration(
                                        "tenant storage is not configured or tenant id is invalid",
                                    )));
                                }
                                let mut tenant_databases =
                                    std::collections::BTreeMap::<String, db::Database>::new();
                                let fingerprint = if tenant_id > 0 {
                                    dever_runtime::tenant::schema_fingerprint()
                                        .map_err(|error| errors.fault(error))?
                                } else {
                                    String::new()
                                };
                                if tenant_id > 0 {
                                    if !definitions.iter().any(|(_, _, tenant, _, _)| *tenant)
                                        && !session
                                            .jobs()
                                            .is_some_and(|jobs| jobs.has_tenant_jobs())
                                    {
                                        return Err(errors.fault(orm::Error::migration(
                                            "application has no tenant Model or Job",
                                        )));
                                    }
                                    if session.has_authorization() {
                                        let connection = session
                                            .settings()
                                            .tenant()
                                            .expect("validated tenant settings")
                                            .database();
                                        let database = dever_runtime::tenant::begin_migration(
                                            connection,
                                            tenant_id,
                                            &fingerprint,
                                        )
                                        .await
                                        .map_err(|error| errors.fault(error))?;
                                        tenant_databases.insert(connection.to_owned(), database);
                                    }
                                }
                                let mut entries = Vec::new();
                                for (explicit, root, tenant, model, error_mapping) in definitions {
                                    if (tenant_id > 0 && !tenant)
                                        || (tenant_id == 0 && tenant && tenant_enabled)
                                    {
                                        continue;
                                    }
                                    let model =
                                        model.map_err(|error| error_mapping.failure(error))?;
                                    let platform = session
                                        .databases()
                                        .database(explicit.as_deref(), &root)
                                        .map_err(|error| error_mapping.fault(error))?;
                                    let database = if tenant_id == 0 {
                                        platform
                                    } else {
                                        let connection = platform.name().to_owned();
                                        if !tenant_databases.contains_key(&connection) {
                                            let database = dever_runtime::tenant::begin_migration(
                                                &connection,
                                                tenant_id,
                                                &fingerprint,
                                            )
                                            .await
                                            .map_err(|error| error_mapping.fault(error))?;
                                            tenant_databases.insert(connection.clone(), database);
                                        }
                                        tenant_databases[&connection].clone()
                                    };
                                    entries.push(migration_entry(database, model, error_mapping));
                                }
                                migrate_entries(entries).await?;
                                if tenant_id > 0 {
                                    if let Some(jobs) = session.jobs() {
                                        jobs.migrate_tenant(
                                            tenant_id,
                                            &fingerprint,
                                            &mut tenant_databases,
                                        )
                                        .await
                                        .map_err(|error| errors.fault(error))?;
                                    }
                                    if session.has_authorization() {
                                        let connection = session
                                            .settings()
                                            .tenant()
                                            .expect("validated tenant settings")
                                            .database();
                                        dever_runtime::auth::store::initialize_roles(
                                            tenant_databases[connection].clone(),
                                        )
                                        .await
                                        .map_err(|error| errors.fault(error))?;
                                    }
                                    for database in tenant_databases.values() {
                                        dever_runtime::tenant::mark_database_ready(
                                            database,
                                            tenant_id,
                                            &fingerprint,
                                        )
                                        .await
                                        .map_err(|error| errors.fault(error))?;
                                    }
                                    dever_runtime::tenant::finish_migration(
                                        tenant_id,
                                        &fingerprint,
                                    )
                                    .await
                                    .map_err(|error| errors.fault(error))?;
                                }
                                Ok(OpValue::Database(Output::Unit))
                            })
                            .await
                    },
                    Some(errors.slots(0, false)),
                )
            }

            /// # Safety
            /// Database is a borrowed owner and errors is static.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_begin(
                database: *const c_void,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                unsafe {
                    operation(
                        errors,
                        buffer,
                        align_of::<*mut c_void>(),
                        false,
                        || cloned::<db::Database>(database),
                        |database| async move {
                            Ok(Output::Transaction(Arc::new(db::TransactionState::new(
                                database.begin().await?,
                            ))))
                        },
                    )
                }
            }
            /// # Safety
            /// Transaction is a borrowed shared state; commit is 0 or 1.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_finish(
                transaction: *const c_void,
                commit: u8,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                unsafe {
                    operation(
                        errors,
                        buffer,
                        0,
                        false,
                        || Ok((cloned::<Transaction>(transaction)?, flag(commit)?)),
                        |(transaction, commit)| async move {
                            transaction.finish(commit).await?;
                            Ok(Output::Unit)
                        },
                    )
                }
            }
            /// # Safety
            /// Primary is an initialized concrete fault, transferred only on non-null
            /// creation. The transaction remains borrowed and is cloned before suspend.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_rollback_take(
                transaction: *const c_void,
                primary: *mut c_void,
                primary_type: *const OwnedType,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(buffer) } {
                    return ptr::null_mut();
                }
                let prepared = (|| {
                    let errors = unsafe { Errors::new(errors) }?;
                    if !std::ptr::eq(primary_type, errors.ty)
                        || primary.is_null()
                        || !(primary as usize).is_multiple_of(errors.align)
                    {
                        return Err(invalid("invalid rollback primary fault"));
                    }
                    let transaction = unsafe { cloned::<Transaction>(transaction) }?;
                    let mut owner = unsafe { Owned::new(primary_type) }.map_err(invalid)?;
                    unsafe { owner.take_from(primary) };
                    Ok((transaction, owner, errors))
                })();
                let (transaction, primary, errors) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { abi_failure(failure, buffer) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        let primary = ForeignFault::Typed(primary);
                        Err(match transaction.finish(false).await {
                            Ok(()) => primary,
                            Err(rollback) => errors.augment(primary, rollback),
                        })
                    },
                    Some(errors.slots(0, false)),
                )
            }

            /// # Safety
            /// SQL is static; parameter handles and database/transaction are borrowed.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_execute(
                database: *const c_void,
                transaction: *const c_void,
                sql: *const Sql,
                parameters: *const Value,
                count: u64,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                unsafe {
                    operation(
                        errors,
                        buffer,
                        align_of::<i64>(),
                        false,
                        || Query::new(database, transaction, sql, parameters, count),
                        |query| async move {
                            let transaction =
                                db::TransactionState::borrow(query.transaction.as_deref()).await?;
                            let executor =
                                db::Executor::new(query.database, transaction.as_deref())?;
                            let result = executor.execute(query.sql, query.parameters).await;
                            drop(executor);
                            Ok(Output::Int(orm::affected(completed(transaction, result)?)?))
                        },
                    )
                }
            }
            /// # Safety
            /// SQL is static; other inputs are cloned before the operation suspends.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_query(
                database: *const c_void,
                transaction: *const c_void,
                sql: *const Sql,
                parameters: *const Value,
                count: u64,
                maximum: i64,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                unsafe {
                    operation(
                        errors,
                        buffer,
                        align_of::<*mut c_void>(),
                        false,
                        || {
                            let maximum = usize::try_from(maximum)
                                .map_err(|_| invalid("invalid database query maximum"))?;
                            Ok((
                                Query::new(database, transaction, sql, parameters, count)?,
                                maximum,
                            ))
                        },
                        |(query, maximum)| async move {
                            let transaction =
                                db::TransactionState::borrow(query.transaction.as_deref()).await?;
                            let executor =
                                db::Executor::new(query.database, transaction.as_deref())?;
                            let result = if maximum == 0 {
                                executor.query(query.sql, query.parameters).await
                            } else {
                                executor
                                    .query_bounded(query.sql, query.parameters, maximum)
                                    .await
                            };
                            drop(executor);
                            Ok(Output::Rows(completed(transaction, result)?))
                        },
                    )
                }
            }
            /// # Safety
            /// SQL fragments are static; IDs and owners are borrowed only until creation.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_relation_query(
                database: *const c_void,
                transaction: *const c_void,
                prefix: *const Name,
                before_limit: *const Name,
                suffix: *const Name,
                ids: *const i64,
                count: u64,
                limit: i64,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                unsafe {
                    operation(
                        errors,
                        buffer,
                        align_of::<*mut c_void>(),
                        false,
                        || {
                            let ids = array(ids, count)?;
                            let sql = db::relation_sql(
                                name(record(prefix)?)?,
                                name(record(before_limit)?)?,
                                name(record(suffix)?)?,
                                ids.len(),
                            );
                            let parameters = ids
                                .iter()
                                .copied()
                                .chain(std::iter::once(limit))
                                .map(orm::Value::Int)
                                .collect::<Vec<_>>();
                            Ok((
                                cloned::<db::Database>(database)?,
                                if transaction.is_null() {
                                    None
                                } else {
                                    Some(cloned::<Transaction>(transaction)?)
                                },
                                sql,
                                parameters,
                            ))
                        },
                        |(database, transaction, sql, parameters)| async move {
                            let transaction =
                                db::TransactionState::borrow(transaction.as_deref()).await?;
                            let executor = db::Executor::new(database, transaction.as_deref())?;
                            let result = executor.query_owned(sql, parameters).await;
                            drop(executor);
                            Ok(Output::Rows(completed(transaction, result)?))
                        },
                    )
                }
            }

            #[derive(Clone)]
            pub(super) struct Stream {
                rows: orm::RowStream<orm::Row>,
                decoder: Decoder,
            }
            #[derive(Clone, Copy)]
            struct Decoder {
                row_type: &'static OwnedType,
                decode: unsafe extern "C" fn(
                    *const c_void,
                    *mut c_void,
                    *const ErrorDescriptor,
                    *mut c_void,
                    *mut Buffer,
                ) -> u32,
            }
            // Decoder contains immutable process-static checked transferable metadata.
            unsafe impl Send for Decoder {}
            unsafe impl Sync for Decoder {}
            impl Decoder {
                unsafe fn new(pointer: *const RowDecoder) -> Result<Self, Failure> {
                    let decoder = unsafe { record(pointer) }?;
                    let (row_type, _) =
                        unsafe { owned_layout(decoder.row_type) }.map_err(invalid)?;
                    if row_type.transferable == 0 {
                        return Err(invalid("database stream row is not transferable"));
                    }
                    Ok(Self {
                        row_type,
                        decode: decoder
                            .decode
                            .ok_or_else(|| invalid("missing database row decoder"))?,
                    })
                }
                fn row(self, row: orm::Row, errors: Errors) -> Result<Owned, ForeignFault> {
                    let mut output =
                        unsafe { Owned::new(self.row_type) }.map_err(ForeignFault::Runtime)?;
                    let mut fault =
                        unsafe { Owned::new(errors.ty) }.map_err(ForeignFault::Runtime)?;
                    let mut buffer = Buffer {
                        ptr: ptr::null_mut(),
                        len: 0,
                    };
                    let status = unsafe {
                        (self.decode)(
                            std::ptr::from_ref(&row).cast(),
                            output.pointer(),
                            errors.descriptor,
                            fault.pointer(),
                            &mut buffer,
                        )
                    };
                    if status == 0 {
                        output.initialized = true;
                    }
                    if status == 1 {
                        fault.initialized = true;
                    }
                    if status <= 1 && buffer.ptr.is_null() && buffer.len == 0 {
                        return if status == 0 {
                            Ok(output)
                        } else {
                            Err(ForeignFault::Typed(fault))
                        };
                    }
                    let message = if buffer.ptr.is_null() {
                        "invalid database row decoder result".to_owned()
                    } else {
                        let bytes =
                            unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len as usize) };
                        let message = String::from_utf8_lossy(bytes).into_owned();
                        unsafe { super::super::dever_rt_v1_buffer_free(buffer.ptr, buffer.len) };
                        message
                    };
                    Err(ForeignFault::Runtime(message))
                }
            }
            /// # Safety
            /// SQL and decoder metadata are static. All borrowed values are cloned now.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_stream(
                database: *const c_void,
                transaction: *const c_void,
                sql: *const Sql,
                parameters: *const Value,
                count: u64,
                capacity: i64,
                decoder: *const RowDecoder,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                unsafe {
                    operation(
                        errors,
                        buffer,
                        align_of::<*mut c_void>(),
                        false,
                        || {
                            Ok((
                                Query::new(database, transaction, sql, parameters, count)?,
                                Decoder::new(decoder)?,
                            ))
                        },
                        move |(query, decoder)| async move {
                            let transaction =
                                db::TransactionState::borrow(query.transaction.as_deref()).await?;
                            let executor =
                                db::Executor::new(query.database, transaction.as_deref())?;
                            let capacity =
                                orm::stream_capacity(capacity, executor.max_page_size())?;
                            let result =
                                executor.stream(query.sql, query.parameters, capacity).await;
                            drop(executor);
                            Ok(Output::Stream(Stream {
                                rows: completed(transaction, result)?,
                                decoder,
                            }))
                        },
                    )
                }
            }
            /// # Safety
            /// Stream is borrowed; output is a writable Unit byte.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_stream_close(
                stream: *const c_void,
                out: *mut u8,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result = unsafe { managed::borrowed::<Stream>(stream) }.map(|stream| {
                    stream.rows.close();
                    0
                });
                unsafe { managed::complete(out, buffer, result) }
            }
            /// # Safety
            /// The stream is borrowed; errors identifies the current consumption site.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_stream_pull(
                stream: *const c_void,
                errors: *const ErrorDescriptor,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(buffer) } {
                    return ptr::null_mut();
                }
                let prepared = (|| {
                    Ok((unsafe { cloned::<Stream>(stream) }?, unsafe {
                        Errors::new(errors)
                    }?))
                })();
                let (stream, errors) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { abi_failure(failure, buffer) };
                        return ptr::null_mut();
                    }
                };
                let align = stream.decoder.row_type.align as usize;
                operation_with_output(
                    async move {
                        let row = stream
                            .rows
                            .pull()
                            .await
                            .map_err(|error| errors.fault(error))?;
                        let decoded = row.map(|row| stream.decoder.row(row, errors)).transpose();
                        if decoded.is_err() {
                            stream.rows.close();
                        }
                        decoded.map(|row| OpValue::Database(Output::Row(row)))
                    },
                    Some(errors.slots(align, true)),
                )
            }

            /// # Safety
            /// Transfers one owned rows collection, or null.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_rows_release(rows: *mut c_void) {
                unsafe { managed::release::<Vec<orm::Row>>(rows) }
            }
            /// # Safety
            /// Transfers one owned database row, or null; decoder callback rows are borrowed.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_row_release(row: *mut c_void) {
                unsafe { managed::release::<orm::Row>(row) }
            }
            macro_rules! length {
                ($function:ident, $ty:ty) => {
                    /// # Safety
                    /// Handle is borrowed and out is a writable Int slot.
                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn $function(
                        handle: *const c_void,
                        out: *mut i64,
                        buffer: *mut Buffer,
                    ) -> u32 {
                        if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                            return status;
                        }
                        let result =
                            unsafe { managed::borrowed::<$ty>(handle) }.and_then(|value| {
                                i64::try_from(value.len())
                                    .map_err(|_| managed::invalid("database row count exceeds Int"))
                            });
                        unsafe { managed::complete(out, buffer, result) }
                    }
                };
            }
            length!(dever_rt_v1_db_rows_len, Vec<orm::Row>);
            length!(dever_rt_v1_db_row_len, orm::Row);
            /// # Safety
            /// Rows is borrowed, index is checked, out receives an independent owned row.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_rows_at(
                rows: *const c_void,
                index: i64,
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result = unsafe { managed::borrowed::<Vec<orm::Row>>(rows) }.and_then(|rows| {
                    usize::try_from(index)
                        .ok()
                        .and_then(|index| rows.get(index))
                        .cloned()
                        .ok_or_else(|| managed::invalid("invalid database row index"))
                });
                unsafe { managed::complete_handle(out, buffer, result) }
            }
            /// # Safety
            /// Rows/operation are borrowed; out and fault are distinct matching destinations.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_required_row(
                rows: *const c_void,
                operation: *const Name,
                out: *mut *mut c_void,
                errors: *const ErrorDescriptor,
                fault: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                unsafe {
                    regular(out, errors, fault, buffer, || {
                        Ok(managed::owned(orm::required_row(
                            cloned(rows)?,
                            name(record(operation)?)?,
                        )?))
                    })
                }
            }
            /// # Safety
            /// The presence slot is distinct; absent success leaves out uninitialized.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_optional_row(
                rows: *const c_void,
                operation: *const Name,
                out: *mut *mut c_void,
                present: *mut u8,
                errors: *const ErrorDescriptor,
                fault: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                let errors = match unsafe { checked(out, errors, fault, buffer) } {
                    Ok(errors) => errors,
                    Err(status) => return status,
                };
                if present.is_null() {
                    return unsafe { error(buffer, 2, "invalid database presence output") };
                }
                let result = (|| {
                    Ok(orm::optional_row(unsafe { cloned(rows) }?, unsafe {
                        name(record(operation)?)
                    }?)?)
                })();
                match result {
                    Ok(value) => {
                        unsafe { present.write(u8::from(value.is_some())) };
                        if let Some(value) = value {
                            unsafe { out.write(managed::owned(value)) };
                        }
                        0
                    }
                    Err(failure) => unsafe {
                        finish::<*mut c_void>(Err(failure), out, errors, fault, buffer)
                    },
                }
            }
            unsafe fn column(row: *const c_void, index: i64) -> Result<orm::Value, Failure> {
                let row = unsafe { managed::borrowed::<orm::Row>(row) }?;
                let index =
                    usize::try_from(index).map_err(|_| invalid("invalid database column index"))?;
                Ok(row.get(index)?.clone())
            }
            macro_rules! getter {
                ($function:ident, $ty:ty, $convert:expr) => {
                    /// # Safety
                    /// Row is borrowed; output/fault slots match their descriptors.
                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn $function(
                        row: *const c_void,
                        index: i64,
                        out: *mut $ty,
                        errors: *const ErrorDescriptor,
                        fault: *mut c_void,
                        buffer: *mut Buffer,
                    ) -> u32 {
                        unsafe {
                            regular(out, errors, fault, buffer, || {
                                let value = column(row, index)?;
                                ($convert)(value).map_err(Failure::from)
                            })
                        }
                    }
                };
            }
            getter!(dever_rt_v1_db_row_null, u8, |value| Ok::<_, orm::Error>(
                u8::from(matches!(value, orm::Value::Null))
            ));
            getter!(dever_rt_v1_db_row_bool, u8, |value| orm::boolean(value)
                .map(u8::from));
            getter!(dever_rt_v1_db_row_int, i64, orm::int);
            getter!(dever_rt_v1_db_row_float, f64, orm::float);
            getter!(dever_rt_v1_db_row_decimal, crate::AbiDecimal, |value| {
                orm::decimal(value).map(crate::AbiDecimal::encode)
            });
            getter!(dever_rt_v1_db_row_text, *mut c_void, |value| orm::text(
                value
            )
            .map(managed::owned));
            getter!(dever_rt_v1_db_row_bytes, *mut c_void, |value| orm::bytes(
                value
            )
            .map(managed::owned));
            getter!(dever_rt_v1_db_row_uuid, *mut c_void, |value| orm::uuid(
                value
            )
            .map(managed::owned));
            /// # Safety
            /// Outputs are distinct matching owned UUID/fault destinations.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_uuid_new(
                out: *mut *mut c_void,
                errors: *const ErrorDescriptor,
                fault: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                unsafe {
                    regular(out, errors, fault, buffer, || {
                        Ok(managed::owned(orm::Uuid::new_v7()?))
                    })
                }
            }
            /// # Safety
            /// Message is borrowed Text and fault matches the static descriptor.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_error(
                kind: u32,
                message: *const c_void,
                errors: *const ErrorDescriptor,
                fault: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                let mut unit = 0u8;
                unsafe {
                    regular(&mut unit, errors, fault, buffer, || {
                        Err::<u8, _>(
                            orm::Error::new(error_kind(kind)?, cloned::<String>(message)?).into(),
                        )
                    })
                }
            }
            /// # Safety
            /// All numeric and fault destinations are distinct writable slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_pagination(
                page: i64,
                size: i64,
                maximum: i64,
                out_page: *mut i64,
                out_size: *mut i64,
                out_offset: *mut i64,
                errors: *const ErrorDescriptor,
                fault: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                let errors = match unsafe { checked(out_page, errors, fault, buffer) } {
                    Ok(errors) => errors,
                    Err(status) => return status,
                };
                if out_offset.is_null()
                    || !out_offset.is_aligned()
                    || out_size.is_null()
                    || !out_size.is_aligned()
                {
                    return unsafe { error(buffer, 2, "invalid pagination output") };
                }
                let result = usize::try_from(maximum)
                    .map_err(|_| invalid("invalid page maximum"))
                    .and_then(|maximum| Ok(orm::pagination(page, size, maximum)?));
                match result {
                    Ok((a, b, c)) => {
                        unsafe {
                            out_page.write(a);
                            out_size.write(b);
                            out_offset.write(c);
                        }
                        0
                    }
                    Err(failure) => unsafe {
                        finish::<i64>(Err(failure), out_page, errors, fault, buffer)
                    },
                }
            }
            /// # Safety
            /// Out/fault destinations match the declared concrete types.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_cursor_size(
                size: i64,
                maximum: i64,
                out: *mut i64,
                errors: *const ErrorDescriptor,
                fault: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                unsafe {
                    regular(out, errors, fault, buffer, || {
                        Ok(orm::cursor_size(
                            size,
                            usize::try_from(maximum)
                                .map_err(|_| invalid("invalid cursor maximum"))?,
                        )?)
                    })
                }
            }
            /// # Safety
            /// Out/fault destinations match the declared concrete types.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_stream_capacity(
                size: i64,
                maximum: i64,
                out: *mut i64,
                errors: *const ErrorDescriptor,
                fault: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                unsafe {
                    regular(out, errors, fault, buffer, || {
                        let capacity = orm::stream_capacity(
                            size,
                            usize::try_from(maximum)
                                .map_err(|_| invalid("invalid stream maximum"))?,
                        )?;
                        i64::try_from(capacity).map_err(|_| invalid("stream capacity exceeds Int"))
                    })
                }
            }
            /// # Safety
            /// Out/fault destinations match the declared concrete types.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_relation_limit(
                maximum: i64,
                out: *mut i64,
                errors: *const ErrorDescriptor,
                fault: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                unsafe {
                    regular(out, errors, fault, buffer, || {
                        Ok(orm::relation_limit(
                            usize::try_from(maximum)
                                .map_err(|_| invalid("invalid relation maximum"))?,
                        )?)
                    })
                }
            }
            /// # Safety
            /// Type describes the borrowed initialized source; out receives one owner.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_related_new(
                ty: *const managed::TypeDescriptor,
                value: *const c_void,
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result = unsafe { managed::descriptor(ty, false) }
                    .and_then(|ops| unsafe { Element::from_source(ops, value) });
                unsafe { managed::complete_handle(out, buffer, result) }
            }
            /// # Safety
            /// Both pointers are matching nullable Related owners.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_related_equal(
                left: *const c_void,
                right: *const c_void,
            ) -> u8 {
                if left.is_null() || right.is_null() {
                    return u8::from(left == right);
                }
                u8::from(unsafe { *left.cast::<Element>() == *right.cast::<Element>() })
            }
            /// # Safety
            /// Related is Loaded; out is uninitialized storage for its exact payload type.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_db_related_get(
                related: *const c_void,
                out: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                if !unsafe { empty(buffer) } {
                    return 2;
                }
                match unsafe { managed::borrowed::<Element>(related) }
                    .and_then(|value| unsafe { value.copy_to(out) })
                {
                    Ok(()) => 0,
                    Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
                }
            }
        }

        mod protocol {
            use super::*;
            use dever_runtime::{http, sse, tls, websocket};

            pub(super) enum Value {
                Response(http::Response),
                StreamResponse(http::StreamResponse),
                Client(http::HttpClient),
                Socket(websocket::WebSocket),
                Message(Option<websocket::Message>),
            }

            impl Value {
                pub(super) unsafe fn write(
                    self,
                    output: *mut c_void,
                    present: *mut u8,
                ) -> Result<(), &'static str> {
                    let output = output.cast::<*mut c_void>();
                    if output.is_null() || !output.is_aligned() {
                        return Err("missing protocol output");
                    }
                    let handle = match self {
                        Self::Response(value) => managed::owned(value),
                        Self::StreamResponse(value) => managed::owned(value),
                        Self::Client(value) => managed::owned(value),
                        Self::Socket(value) => managed::owned(value),
                        Self::Message(value) => {
                            if present.is_null() {
                                return Err("missing message presence output");
                            }
                            unsafe { present.write(u8::from(value.is_some())) };
                            let Some(value) = value else {
                                return Ok(());
                            };
                            managed::owned(value)
                        }
                    };
                    unsafe { output.write(handle) };
                    Ok(())
                }
            }

            unsafe fn cloned<T: Clone>(handle: *const c_void) -> Result<T, managed::AbiFailure> {
                unsafe { managed::borrowed::<T>(handle) }.cloned()
            }

            unsafe fn record<'a, T>(pointer: *const T) -> Result<&'a T, managed::AbiFailure> {
                if pointer.is_null() || !pointer.is_aligned() {
                    return Err(managed::invalid("invalid protocol record"));
                }
                Ok(unsafe { &*pointer })
            }

            macro_rules! owner {
                ($retain:ident, $release:ident, $ty:ty) => {
                    /// # Safety
                    /// handle borrows the matching opaque protocol owner.
                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn $retain(handle: *const c_void) -> *mut c_void {
                        unsafe { managed::retain::<$ty>(handle) }
                    }
                    /// # Safety
                    /// handle transfers one matching protocol owner, or null.
                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn $release(handle: *mut c_void) {
                        unsafe { managed::release::<$ty>(handle) }
                    }
                };
            }
            owner!(
                dever_rt_v1_http_headers_retain,
                dever_rt_v1_http_headers_release,
                Mutex<Vec<http::Header>>
            );
            owner!(
                dever_rt_v1_http_request_retain,
                dever_rt_v1_http_request_release,
                http::Request
            );
            owner!(
                dever_rt_v1_http_response_retain,
                dever_rt_v1_http_response_release,
                http::Response
            );
            owner!(
                dever_rt_v1_http_stream_request_retain,
                dever_rt_v1_http_stream_request_release,
                http::StreamRequest
            );
            owner!(
                dever_rt_v1_http_stream_response_retain,
                dever_rt_v1_http_stream_response_release,
                http::StreamResponse
            );
            owner!(
                dever_rt_v1_client_tls_retain,
                dever_rt_v1_client_tls_release,
                tls::ClientTls
            );
            owner!(
                dever_rt_v1_server_tls_retain,
                dever_rt_v1_server_tls_release,
                tls::ServerTls
            );
            owner!(
                dever_rt_v1_http_client_retain,
                dever_rt_v1_http_client_release,
                http::HttpClient
            );
            owner!(
                dever_rt_v1_http_reply_retain,
                dever_rt_v1_http_reply_release,
                http::HttpReply
            );
            owner!(
                dever_rt_v1_websocket_retain,
                dever_rt_v1_websocket_release,
                websocket::WebSocket
            );
            owner!(
                dever_rt_v1_ws_message_retain,
                dever_rt_v1_ws_message_release,
                websocket::Message
            );

            #[repr(C)]
            #[derive(Clone, Copy)]
            pub(super) struct Http2Limits {
                streams: i64,
                stream_window_bytes: i64,
                connection_window_bytes: i64,
            }
            #[repr(C)]
            pub(super) struct HttpLimits {
                header_bytes: i64,
                body_bytes: i64,
                timeout_ms: i64,
                connections: i64,
                http2: *const Http2Limits,
            }
            impl HttpLimits {
                unsafe fn runtime(&self) -> Result<http::Limits, managed::AbiFailure> {
                    let http2 = if self.http2.is_null() {
                        None
                    } else {
                        let value = unsafe { record(self.http2) }?;
                        Some(http::Http2Limits {
                            streams: value.streams,
                            stream_window_bytes: value.stream_window_bytes,
                            connection_window_bytes: value.connection_window_bytes,
                        })
                    };
                    Ok(http::Limits {
                        header_bytes: self.header_bytes,
                        body_bytes: self.body_bytes,
                        timeout_ms: self.timeout_ms,
                        connections: self.connections,
                        http2,
                    })
                }
            }
            #[repr(C)]
            pub(super) struct PoolLimits {
                idle_ms: i64,
                chunk_bytes: i64,
                read_ms: i64,
            }
            #[repr(C)]
            pub(super) struct LiveLimits {
                chunk_bytes: i64,
                idle_ms: i64,
                heartbeat_ms: i64,
            }
            #[repr(C)]
            pub(super) struct WsLimits {
                message_bytes: i64,
                idle_ms: i64,
                write_ms: i64,
            }
            impl WsLimits {
                fn runtime(&self) -> websocket::Limits {
                    websocket::Limits {
                        message_bytes: self.message_bytes,
                        idle_ms: self.idle_ms,
                        write_ms: self.write_ms,
                    }
                }
            }
            #[repr(C)]
            #[derive(Clone, Copy)]
            pub(super) struct RequestFields {
                method: *mut c_void,
                target: *mut c_void,
                headers: *mut c_void,
                body: *mut c_void,
            }
            #[repr(C)]
            #[derive(Clone, Copy)]
            pub(super) struct ResponseFields {
                status: i64,
                headers: *mut c_void,
                body: *mut c_void,
            }
            #[repr(C)]
            #[derive(Clone, Copy)]
            pub(super) struct HeaderFields {
                name: *mut c_void,
                value: *mut c_void,
            }
            #[repr(C)]
            #[derive(Clone, Copy)]
            pub(super) struct MessageFields {
                kind: u32,
                payload: *mut c_void,
            }
            #[repr(C)]
            pub(super) struct SseEvent {
                event: *const c_void,
                data: *const c_void,
                id: *const c_void,
                retry_ms: i64,
                has_retry: u8,
            }

            unsafe fn headers(
                handle: *const c_void,
            ) -> Result<Vec<http::Header>, managed::AbiFailure> {
                unsafe { managed::borrowed::<Mutex<Vec<http::Header>>>(handle) }?
                    .lock()
                    .map(|headers| headers.clone())
                    .map_err(|_| managed::invalid("HTTP headers lock failed"))
            }

            /// # Safety
            /// out/error are distinct writable slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_http_headers_new(
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                unsafe {
                    managed::complete_handle(
                        out,
                        buffer,
                        Ok(Mutex::new(Vec::<http::Header>::new())),
                    )
                }
            }
            /// # Safety
            /// Handles borrow matching header builder/Text/Bytes; buffer is writable.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_http_headers_push(
                handle: *mut c_void,
                name: *const c_void,
                value: *const c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                if !unsafe { empty(buffer) } {
                    return ABI_INVALID_INPUT;
                }
                let result: Result<(), managed::AbiFailure> = (|| {
                    let header = http::Header {
                        name: unsafe { cloned(name) }?,
                        value: unsafe { cloned(value) }?,
                    };
                    unsafe { managed::borrowed::<Mutex<Vec<http::Header>>>(handle) }?
                        .lock()
                        .map_err(|_| managed::invalid("HTTP headers lock failed"))?
                        .push(header);
                    Ok(())
                })();
                match result {
                    Ok(()) => 0,
                    Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
                }
            }
            /// # Safety
            /// headers is borrowed; output/error are distinct writable slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_http_headers_length(
                handle: *const c_void,
                out: *mut i64,
                buffer: *mut Buffer,
            ) -> u32 {
                let result = (|| {
                    let headers = unsafe { managed::borrowed::<Mutex<Vec<http::Header>>>(handle) }?
                        .lock()
                        .map_err(|_| managed::invalid("HTTP headers lock failed"))?;
                    Ok(headers.len() as i64)
                })();
                unsafe { managed::complete(out, buffer, result) }
            }
            /// # Safety
            /// headers is borrowed; output/error are distinct writable slots. Returned fields are owned.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_http_headers_get(
                handle: *const c_void,
                index: i64,
                out: *mut HeaderFields,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result = (|| {
                    let headers = unsafe { managed::borrowed::<Mutex<Vec<http::Header>>>(handle) }?
                        .lock()
                        .map_err(|_| managed::invalid("HTTP headers lock failed"))?;
                    let header = usize::try_from(index)
                        .ok()
                        .and_then(|index| headers.get(index))
                        .ok_or_else(|| managed::invalid("invalid HTTP header index"))?;
                    Ok(HeaderFields {
                        name: managed::owned(header.name.clone()),
                        value: managed::owned(header.value.clone()),
                    })
                })();
                unsafe { managed::complete(out, buffer, result) }
            }

            /// # Safety
            /// fields borrow canonical Text/header/Bytes handles; outputs are distinct writable slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_http_request_new(
                fields: *const RequestFields,
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                let result = (|| {
                    let fields = unsafe { record(fields) }?;
                    Ok(http::Request {
                        method: unsafe { cloned(fields.method) }?,
                        target: unsafe { cloned(fields.target) }?,
                        headers: unsafe { headers(fields.headers) }?,
                        body: unsafe { cloned(fields.body) }?,
                    })
                })();
                unsafe { managed::complete_handle(out, buffer, result) }
            }
            /// # Safety
            /// request is borrowed; outputs are distinct writable slots. Returned fields are owned.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_http_request_fields(
                request: *const c_void,
                out: *mut RequestFields,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result = unsafe { managed::borrowed::<http::Request>(request) }.map(|value| {
                    RequestFields {
                        method: managed::owned(value.method.clone()),
                        target: managed::owned(value.target.clone()),
                        headers: managed::owned(Mutex::new(value.headers.clone())),
                        body: managed::owned(value.body.clone()),
                    }
                });
                unsafe { managed::complete(out, buffer, result) }
            }
            /// # Safety
            /// fields borrow canonical header/Bytes handles; outputs are distinct writable slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_http_response_new(
                fields: *const ResponseFields,
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                let result = (|| {
                    let fields = unsafe { record(fields) }?;
                    Ok(http::Response {
                        status: fields.status,
                        headers: unsafe { headers(fields.headers) }?,
                        body: unsafe { cloned(fields.body) }?,
                    })
                })();
                unsafe { managed::complete_handle(out, buffer, result) }
            }
            /// # Safety
            /// response is borrowed; outputs are distinct writable slots. Returned fields are owned.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_http_response_fields(
                response: *const c_void,
                out: *mut ResponseFields,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result =
                    unsafe { managed::borrowed::<http::Response>(response) }.map(|value| {
                        ResponseFields {
                            status: value.status,
                            headers: managed::owned(Mutex::new(value.headers.clone())),
                            body: managed::owned(value.body.clone()),
                        }
                    });
                unsafe { managed::complete(out, buffer, result) }
            }

            /// # Safety
            /// output/error are distinct writable slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_tls_system(
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                unsafe { managed::complete_handle(out, buffer, Ok(tls::system())) }
            }
            /// # Safety
            /// ca borrows Bytes; output/error are distinct writable slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_tls_client(
                ca: *const c_void,
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                let result = unsafe { managed::borrowed::<Bytes>(ca) }
                    .and_then(|ca| tls::client(ca).map_err(managed::runtime_error));
                unsafe { managed::complete_handle(out, buffer, result) }
            }
            /// # Safety
            /// cert/key borrow Bytes; output/error are distinct writable slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_tls_server(
                cert: *const c_void,
                key: *const c_void,
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                let result = (|| {
                    tls::server(unsafe { managed::borrowed(cert) }?, unsafe {
                        managed::borrowed(key)
                    }?)
                    .map_err(managed::runtime_error)
                })();
                unsafe { managed::complete_handle(out, buffer, result) }
            }

            type DecodeChunk = unsafe extern "C" fn(*const c_void, *mut *mut c_void) -> u32;

            /// # Safety
            /// fields borrow canonical Text/header/AsyncStream handles; decoder is static
            /// and returns one owned Bytes or Text handle for each matching event.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_http_stream_request_new(
                fields: *const RequestFields,
                decode: Option<DecodeChunk>,
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result = (|| {
                    let fields = unsafe { record(fields) }?;
                    let decode =
                        decode.ok_or_else(|| managed::invalid("missing upload event decoder"))?;
                    let stream = unsafe { managed::borrowed::<AsyncStreamHandle>(fields.body) }?
                        .0
                        .clone();
                    Ok(http::StreamRequest {
                        method: unsafe { cloned(fields.method) }?,
                        target: unsafe { cloned(fields.target) }?,
                        headers: unsafe { headers(fields.headers) }?,
                        body: stream.map(move |row| {
                            let mut handle = ptr::null_mut();
                            let status = unsafe { decode(row.pointer(), &mut handle) };
                            match status {
                                0 => {
                                    let value = unsafe { cloned::<Bytes>(handle) }
                                        .map_err(|failure| failure.message);
                                    unsafe { managed::release::<Bytes>(handle) };
                                    value
                                }
                                1 => {
                                    let message = unsafe { cloned::<String>(handle) }
                                        .unwrap_or_else(|failure| failure.message);
                                    unsafe { managed::release::<String>(handle) };
                                    Err(message)
                                }
                                _ => Err("invalid upload event decoder status".into()),
                            }
                        }),
                    })
                })();
                unsafe { managed::complete_handle(out, buffer, result) }
            }

            /// # Safety
            /// callbacks match the checked stream event type; returned fields own handles.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_http_stream_response_fields(
                response: *const c_void,
                event: *const managed::ReadEventDescriptor,
                ty: *const OwnedType,
                out: *mut ResponseFields,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result = (|| {
                    let event = unsafe { StreamEvent::new(event, ty) }?;
                    let response = unsafe { managed::borrowed::<http::StreamResponse>(response) }?;
                    let body =
                        AsyncStreamHandle(response.body.clone().map(move |value| event.row(value)));
                    Ok(ResponseFields {
                        status: response.status,
                        headers: managed::owned(Mutex::new(response.headers.clone())),
                        body: managed::owned(body),
                    })
                })();
                unsafe { managed::complete(out, buffer, result) }
            }

            /// # Safety
            /// payload borrows Text for kind=0, Bytes for kinds=1..3.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_ws_message_new(
                kind: u32,
                payload: *const c_void,
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                let result = match kind {
                    0 => unsafe { cloned(payload) }.map(websocket::Message::Text),
                    1 => unsafe { cloned(payload) }.map(websocket::Message::Binary),
                    2 => unsafe { cloned(payload) }.map(websocket::Message::Ping),
                    3 => unsafe { cloned(payload) }.map(websocket::Message::Pong),
                    _ => Err(managed::invalid("invalid WebSocket message kind")),
                };
                unsafe { managed::complete_handle(out, buffer, result) }
            }
            /// # Safety
            /// message is borrowed; returned payload owns Text or Bytes according to kind.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_ws_message_fields(
                message: *const c_void,
                out: *mut MessageFields,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result =
                    unsafe { managed::borrowed::<websocket::Message>(message) }.map(|message| {
                        match message {
                            websocket::Message::Text(value) => MessageFields {
                                kind: 0,
                                payload: managed::owned(value.clone()),
                            },
                            websocket::Message::Binary(value) => MessageFields {
                                kind: 1,
                                payload: managed::owned(value.clone()),
                            },
                            websocket::Message::Ping(value) => MessageFields {
                                kind: 2,
                                payload: managed::owned(value.clone()),
                            },
                            websocket::Message::Pong(value) => MessageFields {
                                kind: 3,
                                payload: managed::owned(value.clone()),
                            },
                        }
                    });
                unsafe { managed::complete(out, buffer, result) }
            }
            #[repr(C)]
            pub(super) struct WsEvent {
                element_type: *const managed::TypeDescriptor,
                read: Option<unsafe extern "C" fn(u32, *const c_void, *mut c_void)>,
                failed: Option<unsafe extern "C" fn(*const c_void, *mut c_void)>,
            }
            fn message_row(
                message: websocket::Message,
                element: managed::ValueOps,
                read: unsafe extern "C" fn(u32, *const c_void, *mut c_void),
            ) -> StreamRow {
                fn initialize<T>(
                    kind: u32,
                    payload: T,
                    element: managed::ValueOps,
                    read: unsafe extern "C" fn(u32, *const c_void, *mut c_void),
                ) -> StreamRow {
                    let payload = managed::owned(payload);
                    let row = Element::from_initializer(element, |out| unsafe {
                        read(kind, payload, out)
                    });
                    unsafe { managed::release::<T>(payload) };
                    StreamRow::Managed(row)
                }
                match message {
                    websocket::Message::Text(value) => initialize(0, value, element, read),
                    websocket::Message::Binary(value) => initialize(1, value, element, read),
                    websocket::Message::Ping(value) => initialize(2, value, element, read),
                    websocket::Message::Pong(value) => initialize(3, value, element, read),
                }
            }
            /// # Safety
            /// socket is borrowed; callbacks initialize the exact transferable event type.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_ws_messages(
                socket: *const c_void,
                event: *const WsEvent,
                ty: *const OwnedType,
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(status) = unsafe { managed::checked_output(out, buffer) } {
                    return status;
                }
                let result = (|| {
                    let event = unsafe { record(event) }?;
                    let element = unsafe { managed::descriptor(event.element_type, false) }?;
                    unsafe { stream_element_type(ty, element.size, element.align) }?;
                    let read = event
                        .read
                        .ok_or_else(|| managed::invalid("missing WebSocket read callback"))?;
                    let failed = event
                        .failed
                        .ok_or_else(|| managed::invalid("missing WebSocket failed callback"))?;
                    let socket = unsafe { cloned::<websocket::WebSocket>(socket) }?;
                    Ok(AsyncStreamHandle(websocket::messages(socket).map(
                        move |value| match value {
                            Ok(message) => message_row(message, element, read),
                            Err(message) => StreamRow::Managed(managed::initialize_event(
                                message, element, failed,
                            )),
                        },
                    )))
                })();
                unsafe { managed::complete_handle(out, buffer, result) }
            }

            type PackRequest = unsafe extern "C" fn(
                *const c_void,
                *const c_void,
                *const c_void,
                *mut c_void,
                *mut Buffer,
            ) -> u32;
            type ConvertHandle =
                unsafe extern "C" fn(*const c_void, *mut *mut c_void, *mut Buffer) -> u32;
            #[repr(C)]
            pub(super) struct HttpHandler {
                asynchronous: *const AsyncFunction,
                synchronous: *const SyncFunction,
                input_type: *const OwnedType,
                context_type: *const managed::TypeDescriptor,
                pack: Option<PackRequest>,
                response: Option<ConvertHandle>,
                fault_message: Option<ConvertHandle>,
            }
            struct Handler {
                asynchronous: Option<&'static AsyncFunction>,
                synchronous: Option<&'static SyncFunction>,
                input_type: &'static OwnedType,
                context: Option<Element>,
                pack: PackRequest,
                response: Option<ConvertHandle>,
                fault_message: ConvertHandle,
            }
            // Checked process-static callbacks only access transferable input/context,
            // and create a separate owned frame/result/fault for every request.
            unsafe impl Send for Handler {}
            unsafe impl Sync for Handler {}

            fn callback_error(status: u32, buffer: Buffer) -> Result<(), String> {
                if status == 0 && buffer.ptr.is_null() {
                    return Ok(());
                }
                let message = if buffer.ptr.is_null() {
                    "protocol callback failed without a diagnostic".to_owned()
                } else {
                    let bytes =
                        unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len as usize) };
                    let message = String::from_utf8_lossy(bytes).into_owned();
                    unsafe { super::super::dever_rt_v1_buffer_free(buffer.ptr, buffer.len) };
                    message
                };
                Err(message)
            }
            fn callback_buffer() -> Buffer {
                Buffer {
                    ptr: ptr::null_mut(),
                    len: 0,
                }
            }

            impl Handler {
                unsafe fn new(
                    descriptor: *const HttpHandler,
                    context: *const c_void,
                    live: bool,
                ) -> Result<Self, managed::AbiFailure> {
                    let descriptor = unsafe { record(descriptor) }?;
                    let asynchronous = if descriptor.asynchronous.is_null() {
                        None
                    } else {
                        Some(unsafe { record(descriptor.asynchronous) }?)
                    };
                    let synchronous = if descriptor.synchronous.is_null() {
                        None
                    } else {
                        Some(unsafe { record(descriptor.synchronous) }?)
                    };
                    let (send_safe, output_type, fault_type) = match (asynchronous, synchronous) {
                        (Some(function), None)
                            if function.create.is_some()
                                && function.resume.is_some()
                                && function.destroy.is_some()
                                && function.done.is_some() =>
                        {
                            (
                                function.send_safe,
                                function.output_type,
                                function.fault_type,
                            )
                        }
                        (None, Some(function))
                            if function.input_type == descriptor.input_type
                                && function.invoke.is_some() =>
                        {
                            (
                                function.send_safe,
                                function.output_type,
                                function.fault_type,
                            )
                        }
                        _ => return Err(managed::invalid("invalid HTTP handler function")),
                    };
                    for ty in [descriptor.input_type, output_type, fault_type] {
                        let (ty, _) = unsafe { owned_layout(ty) }.map_err(managed::invalid)?;
                        if send_safe == 0 || ty.transferable == 0 {
                            return Err(managed::invalid("HTTP handler is not transferable"));
                        }
                    }
                    if live && unsafe { &*output_type }.size != 0 {
                        return Err(managed::invalid("live HTTP handler must return Unit"));
                    }
                    if !live && descriptor.response.is_none() {
                        return Err(managed::invalid("missing HTTP response callback"));
                    }
                    let context = if descriptor.context_type.is_null() {
                        None
                    } else {
                        let ops = unsafe { managed::descriptor(descriptor.context_type, false) }?;
                        Some(unsafe { Element::from_source(ops, context) }?)
                    };
                    Ok(Self {
                        asynchronous,
                        synchronous,
                        input_type: unsafe { &*descriptor.input_type },
                        context,
                        pack: descriptor
                            .pack
                            .ok_or_else(|| managed::invalid("missing HTTP request callback"))?,
                        response: descriptor.response,
                        fault_message: descriptor
                            .fault_message
                            .ok_or_else(|| managed::invalid("missing HTTP fault callback"))?,
                    })
                }

                fn input(
                    &self,
                    request: http::Request,
                    reply: Option<http::HttpReply>,
                ) -> Result<Owned, String> {
                    let mut input = unsafe { Owned::new(self.input_type) }?;
                    let request = managed::owned(request);
                    let reply = reply.map_or(ptr::null_mut(), managed::owned);
                    let context = self.context.as_ref().map_or(ptr::null(), Element::pointer);
                    let mut buffer = callback_buffer();
                    let status = unsafe {
                        (self.pack)(request, reply, context, input.pointer(), &mut buffer)
                    };
                    unsafe {
                        managed::release::<http::Request>(request);
                        managed::release::<http::HttpReply>(reply);
                    }
                    callback_error(status, buffer)?;
                    input.initialized = true;
                    Ok(input)
                }

                fn terminal_fault(&self, fault: ForeignFault) -> String {
                    match fault {
                        ForeignFault::Runtime(message) => message,
                        ForeignFault::Typed(fault) => {
                            let mut handle = ptr::null_mut();
                            let mut buffer = callback_buffer();
                            let status = unsafe {
                                (self.fault_message)(fault.pointer(), &mut handle, &mut buffer)
                            };
                            if let Err(message) = callback_error(status, buffer) {
                                return message;
                            }
                            let message = unsafe { cloned::<String>(handle) }
                                .unwrap_or_else(|failure| failure.message);
                            unsafe { managed::release::<String>(handle) };
                            message
                        }
                    }
                }

                async fn invoke(
                    &self,
                    request: http::Request,
                    reply: Option<http::HttpReply>,
                ) -> Result<Owned, String> {
                    let input = self.input(request, reply)?;
                    let result = if let Some(function) = self.asynchronous {
                        let future = unsafe { ForeignFuture::new(function, input.pointer()) }
                            .map(SendForeignFuture)?;
                        drop(input);
                        future.await
                    } else {
                        let function = self.synchronous.expect("validated handler");
                        let prepared = PreparedSync {
                            input,
                            output: unsafe { Owned::new(function.output_type) }?,
                            fault: unsafe { Owned::new(function.fault_type) }?,
                            invoke: function.invoke.expect("validated handler"),
                            unit_output: false,
                        };
                        prepared.execute()
                    };
                    result.map_err(|fault| self.terminal_fault(fault))
                }

                fn response(&self, output: Owned) -> Result<http::Response, String> {
                    let mut handle = ptr::null_mut();
                    let mut buffer = callback_buffer();
                    let status = unsafe {
                        (self.response.expect("buffered handler"))(
                            output.pointer(),
                            &mut handle,
                            &mut buffer,
                        )
                    };
                    callback_error(status, buffer)?;
                    let response = unsafe { cloned::<http::Response>(handle) }
                        .map_err(|failure| failure.message);
                    unsafe { managed::release::<http::Response>(handle) };
                    response
                }
            }

            /// # Safety
            /// All inputs borrow canonical owners and static checked handler descriptors.
            /// The constructor clones context before return; pending cancel drops all owners.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_async_http_serve(
                listener: *const c_void,
                limits: *const HttpLimits,
                live: *const LiveLimits,
                tls: *const c_void,
                descriptor: *const HttpHandler,
                context: *const c_void,
                buffer: *mut Buffer,
            ) -> *mut AsyncOp {
                if !unsafe { empty(buffer) } {
                    return ptr::null_mut();
                }
                let prepared = (|| {
                    let listener = unsafe { cloned::<net::Listener>(listener) }?;
                    let limits = unsafe { record(limits)?.runtime() }?;
                    let live = if live.is_null() {
                        None
                    } else {
                        let live = unsafe { record(live) }?;
                        Some(http::LiveLimits {
                            chunk_bytes: live.chunk_bytes,
                            idle_ms: live.idle_ms,
                            heartbeat_ms: live.heartbeat_ms,
                        })
                    };
                    let tls = if tls.is_null() {
                        None
                    } else {
                        Some(unsafe { cloned::<tls::ServerTls>(tls) }?)
                    };
                    let handler =
                        Arc::new(unsafe { Handler::new(descriptor, context, live.is_some()) }?);
                    Ok::<_, managed::AbiFailure>((listener, limits, live, tls, handler))
                })();
                let (listener, limits, live, tls, handler) = match prepared {
                    Ok(value) => value,
                    Err(failure) => {
                        unsafe { error(buffer, failure.status, failure.message) };
                        return ptr::null_mut();
                    }
                };
                operation_with_output(
                    async move {
                        let result = if let Some(live) = live {
                            let route = move |request, reply| {
                                let handler = handler.clone();
                                async move { handler.invoke(request, Some(reply)).await.map(drop) }
                            };
                            match tls {
                                Some(tls) => {
                                    http::serve_live_tls(route, listener, limits, live, tls).await
                                }
                                None => http::serve_live(route, listener, limits, live).await,
                            }
                        } else {
                            let route = move |request| {
                                let handler = handler.clone();
                                async move {
                                    let output = handler.invoke(request, None).await?;
                                    handler.response(output)
                                }
                            };
                            match tls {
                                Some(tls) => http::serve_tls(route, listener, limits, tls).await,
                                None => http::serve(route, listener, limits).await,
                            }
                        };
                        result.map(|()| OpValue::Unit).map_err(ForeignFault::from)
                    },
                    Some(OutputSlots::Unit),
                )
            }

            // Constructors copy all borrowed inputs before returning, including
            // records nested beneath config pointers. Futures only capture owners.
            macro_rules! protocol_op {
                ($slots:ident, $name:ident($($arg:ident: $ty:ty),*) $prepare:block => |$input:pat_param| $future:expr) => {
                    /// # Safety
                    /// Inputs borrow the canonical types in runtime.h through this call only.
                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn $name($($arg:$ty),*) -> *mut AsyncOp {
                        let prepare = || $prepare;
                        let prepared: Result<_, managed::AbiFailure> = prepare();
                        let Ok($input) = prepared else { return ptr::null_mut(); };
                        operation_with_output($future, Some(OutputSlots::$slots))
                    }
                };
            }
            protocol_op!(Handle, dever_rt_v1_async_http_send(host: *const c_void, port: i64, request: *const c_void, limits: *const HttpLimits) {
                Ok((unsafe { cloned::<String>(host) }?, unsafe { cloned::<http::Request>(request) }?, unsafe { record(limits)?.runtime() }?))
            } => |(host, request, limits)| async move { http::send(&host, port, request, limits).await.map(|value| OpValue::Protocol(Value::Response(value))).map_err(ForeignFault::from) });
            protocol_op!(Handle, dever_rt_v1_async_http_client(origin: *const c_void, tls: *const c_void, limits: *const HttpLimits, pool: *const PoolLimits) {
                let pool = unsafe { record(pool) }?;
                Ok((unsafe { cloned::<String>(origin) }?, unsafe { cloned::<tls::ClientTls>(tls) }?, unsafe { record(limits)?.runtime() }?, http::PoolLimits { idle_ms: pool.idle_ms, chunk_bytes: pool.chunk_bytes, read_ms: pool.read_ms }))
            } => |(origin, tls, limits, pool)| async move { http::client(&origin, tls, limits, pool).await.map(|value| OpValue::Protocol(Value::Client(value))).map_err(ForeignFault::from) });
            protocol_op!(Handle, dever_rt_v1_async_http_request(client: *const c_void, request: *const c_void) {
                Ok((unsafe { cloned::<http::HttpClient>(client) }?, unsafe { cloned::<http::Request>(request) }?))
            } => |(client, request)| async move { http::request(&client, request).await.map(|value| OpValue::Protocol(Value::Response(value))).map_err(ForeignFault::from) });
            protocol_op!(Handle, dever_rt_v1_async_http_open(client: *const c_void, request: *const c_void) {
                Ok((unsafe { cloned::<http::HttpClient>(client) }?, unsafe { cloned::<http::Request>(request) }?))
            } => |(client, request)| async move { http::open(&client, request).await.map(|value| OpValue::Protocol(Value::StreamResponse(value))).map_err(ForeignFault::from) });
            protocol_op!(Handle, dever_rt_v1_async_http_open_stream(client: *const c_void, request: *const c_void) {
                Ok((unsafe { cloned::<http::HttpClient>(client) }?, unsafe { cloned::<http::StreamRequest>(request) }?))
            } => |(client, request)| async move { http::open_stream(&client, request).await.map(|value| OpValue::Protocol(Value::StreamResponse(value))).map_err(ForeignFault::from) });
            protocol_op!(Bool, dever_rt_v1_async_http_close_client(client: *const c_void) { unsafe { cloned::<http::HttpClient>(client) } }
                => |client| async move { http::close_client(&client).await.map(|()| OpValue::Bool(true)).map_err(ForeignFault::from) });
            protocol_op!(Bool, dever_rt_v1_async_http_respond(reply: *const c_void, response: *const c_void) {
                Ok((unsafe { cloned::<http::HttpReply>(reply) }?, unsafe { cloned::<http::Response>(response) }?))
            } => |(reply, response)| async move { http::respond(&reply, response).await.map(|()| OpValue::Bool(true)).map_err(ForeignFault::from) });
            protocol_op!(Bool, dever_rt_v1_async_http_start(reply: *const c_void, status: i64, header_values: *const c_void) {
                Ok((unsafe { cloned::<http::HttpReply>(reply) }?, unsafe { headers(header_values) }?))
            } => |(reply, headers)| async move { http::start(&reply, status, headers).await.map(|()| OpValue::Bool(true)).map_err(ForeignFault::from) });
            protocol_op!(Bool, dever_rt_v1_async_http_write(reply: *const c_void, bytes: *const c_void) {
                Ok((unsafe { cloned::<http::HttpReply>(reply) }?, unsafe { cloned::<Bytes>(bytes) }?))
            } => |(reply, bytes)| async move { http::write(&reply, bytes).await.map(|()| OpValue::Bool(true)).map_err(ForeignFault::from) });
            protocol_op!(Bool, dever_rt_v1_async_http_finish(reply: *const c_void) { unsafe { cloned::<http::HttpReply>(reply) } }
                => |reply| async move { http::finish(&reply).await.map(|()| OpValue::Bool(true)).map_err(ForeignFault::from) });
            protocol_op!(Bool, dever_rt_v1_async_sse_start(reply: *const c_void, header_values: *const c_void) {
                Ok((unsafe { cloned::<http::HttpReply>(reply) }?, unsafe { headers(header_values) }?))
            } => |(reply, headers)| async move { sse::start(&reply, headers).await.map(|()| OpValue::Bool(true)).map_err(ForeignFault::from) });
            protocol_op!(Bool, dever_rt_v1_async_sse_send(reply: *const c_void, event: *const SseEvent) {
                let event = unsafe { record(event) }?;
                Ok((unsafe { cloned::<http::HttpReply>(reply) }?, sse::Event { event: unsafe { cloned(event.event) }?, data: unsafe { cloned(event.data) }?, id: if event.id.is_null() { None } else { Some(unsafe { cloned(event.id) }?) }, retry_ms: (event.has_retry != 0).then_some(event.retry_ms) }))
            } => |(reply, event)| async move { sse::send(&reply, event).await.map(|()| OpValue::Bool(true)).map_err(ForeignFault::from) });
            protocol_op!(Handle, dever_rt_v1_async_ws_accept(reply: *const c_void, limits: *const WsLimits) {
                Ok((unsafe { cloned::<http::HttpReply>(reply) }?, unsafe { record(limits) }?.runtime()))
            } => |(reply, limits)| async move { websocket::accept(&reply, limits).await.map(|value| OpValue::Protocol(Value::Socket(value))).map_err(ForeignFault::from) });
            protocol_op!(Handle, dever_rt_v1_async_ws_connect(host: *const c_void, port: i64, target: *const c_void, limits: *const WsLimits) {
                Ok((unsafe { cloned::<String>(host) }?, unsafe { cloned::<String>(target) }?, unsafe { record(limits) }?.runtime()))
            } => |(host, target, limits)| async move { websocket::connect(&host, port, &target, limits).await.map(|value| OpValue::Protocol(Value::Socket(value))).map_err(ForeignFault::from) });
            protocol_op!(Handle, dever_rt_v1_async_ws_open(url: *const c_void, tls: *const c_void, limits: *const WsLimits) {
                Ok((unsafe { cloned::<String>(url) }?, unsafe { cloned::<tls::ClientTls>(tls) }?, unsafe { record(limits) }?.runtime()))
            } => |(url, tls, limits)| async move { websocket::open(&url, tls, limits).await.map(|value| OpValue::Protocol(Value::Socket(value))).map_err(ForeignFault::from) });
            protocol_op!(Bool, dever_rt_v1_async_ws_send(socket: *const c_void, message: *const c_void) {
                Ok((unsafe { cloned::<websocket::WebSocket>(socket) }?, unsafe { cloned::<websocket::Message>(message) }?))
            } => |(socket, message)| async move { websocket::send(&socket, message).await.map(|()| OpValue::Bool(true)).map_err(ForeignFault::from) });
            protocol_op!(OptionalHandle, dever_rt_v1_async_ws_receive(socket: *const c_void) { unsafe { cloned::<websocket::WebSocket>(socket) } }
                => |socket| async move { websocket::receive(&socket).await.map(|value| OpValue::Protocol(Value::Message(value))).map_err(ForeignFault::from) });
            protocol_op!(Bool, dever_rt_v1_async_ws_close(socket: *const c_void, code: i64, reason: *const c_void) {
                Ok((unsafe { cloned::<websocket::WebSocket>(socket) }?, unsafe { cloned::<String>(reason) }?))
            } => |(socket, reason)| async move { websocket::close(&socket, code, &reason).await.map(|()| OpValue::Bool(true)).map_err(ForeignFault::from) });
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn dever_rt_v1_async_frame_alloc(size: u64, align: u64) -> *mut c_void {
            let size = usize::try_from(size).unwrap_or_else(|_| std::process::abort());
            let align = usize::try_from(align).unwrap_or_else(|_| std::process::abort());
            let layout = Layout::from_size_align(size.max(1), align)
                .unwrap_or_else(|_| std::process::abort());
            // SAFETY: layout is valid and the matching free receives it.
            NonNull::new(unsafe { alloc(layout) })
                .unwrap_or_else(|| handle_alloc_error(layout))
                .as_ptr()
                .cast()
        }

        /// # Safety
        /// ptr, size and align are one exact frame_alloc allocation, consumed
        /// once. A null ptr is a no-op for allocation-elided coroutines.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_async_frame_free(
            ptr: *mut c_void,
            size: u64,
            align: u64,
        ) {
            if ptr.is_null() {
                return;
            }
            let Ok(size) = usize::try_from(size) else {
                return;
            };
            let Ok(align) = usize::try_from(align) else {
                return;
            };
            let Ok(layout) = Layout::from_size_align(size.max(1), align) else {
                return;
            };
            // SAFETY: the callback promises the exact allocation identity.
            unsafe { dealloc(ptr.cast(), layout) };
        }
    }

    #[repr(C)]
    pub(super) struct Buffer {
        ptr: *mut u8,
        len: u64,
    }

    unsafe fn empty(buffer: *mut Buffer) -> bool {
        if buffer.is_null() || !buffer.is_aligned() {
            return false;
        }
        // SAFETY: the exported caller promises a live, writable, aligned buffer.
        // Every call initializes it before returning, including error paths.
        unsafe {
            buffer.write(Buffer {
                ptr: ptr::null_mut(),
                len: 0,
            });
        }
        true
    }

    unsafe fn owned_bytes(buffer: *mut Buffer, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let boxed = bytes.into_boxed_slice();
        let len = boxed.len() as u64;
        let ptr = Box::into_raw(boxed) as *mut u8;
        // SAFETY: empty() validated and initialized this caller-owned out slot.
        // The allocation itself remains owned by buffer_free until called once.
        unsafe { buffer.write(Buffer { ptr, len }) };
    }

    unsafe fn error(buffer: *mut Buffer, status: u32, message: impl Into<String>) -> u32 {
        // SAFETY: this slot was validated by empty() before the call.
        unsafe { owned_bytes(buffer, message.into().into_bytes()) };
        status
    }

    unsafe fn value<T: Copy>(
        out: *mut T,
        buffer: *mut Buffer,
        result: Result<T, &'static str>,
    ) -> u32 {
        // SAFETY: the exported caller promises a live Buffer slot.
        if !unsafe { empty(buffer) } {
            return ABI_INVALID_INPUT;
        }
        if out.is_null() || !out.is_aligned() {
            // SAFETY: empty() validated and initialized the Buffer slot.
            return unsafe { error(buffer, ABI_INVALID_INPUT, "invalid ABI output pointer") };
        }
        match result {
            Ok(value) => {
                // SAFETY: the caller promises one live, writable, aligned T slot,
                // disjoint from the Buffer slot; it is initialized only on success.
                unsafe { out.write(value) };
                ABI_OK
            }
            // SAFETY: empty() validated and initialized the Buffer slot.
            Err(message) => unsafe { error(buffer, ABI_RUNTIME_ERROR, message) },
        }
    }

    unsafe fn text(buffer: *mut Buffer, result: Result<Vec<u8>, String>) -> u32 {
        // SAFETY: the exported caller promises a live Buffer slot.
        if !unsafe { empty(buffer) } {
            return ABI_INVALID_INPUT;
        }
        match result {
            Ok(bytes) => {
                // SAFETY: empty() validated and initialized the Buffer slot.
                unsafe { owned_bytes(buffer, bytes) };
                ABI_OK
            }
            // SAFETY: empty() validated and initialized the Buffer slot.
            Err(message) => unsafe { error(buffer, ABI_RUNTIME_ERROR, message) },
        }
    }

    /// # Safety
    /// `ptr` must address `len` live bytes for this call when `len > 0`.
    unsafe fn input<'a>(ptr: *const u8, len: u64) -> Result<&'a [u8], &'static str> {
        let len = usize::try_from(len).map_err(|_| "invalid ABI input length")?;
        if len > isize::MAX as usize {
            return Err("invalid ABI input length");
        }
        if len == 0 {
            return Ok(&[]);
        }
        if ptr.is_null() {
            return Err("invalid ABI input pointer");
        }
        // SAFETY: the caller owns a live allocation of at least len bytes
        // and does not mutate it until the returned slice is no longer used.
        Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn dever_rt_v1_version() -> u32 {
        crate::RUNTIME_ABI_VERSION
    }

    /// # Safety
    /// out and buffer are live, writable, aligned, and disjoint. A returned
    /// error buffer is released through dever_rt_v1_buffer_free.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_time_sleep(
        milliseconds: i64,
        out: *mut u8,
        buffer: *mut Buffer,
    ) -> u32 {
        if !unsafe { empty(buffer) } {
            return ABI_INVALID_INPUT;
        }
        if out.is_null() {
            return unsafe { error(buffer, ABI_INVALID_INPUT, "invalid ABI output pointer") };
        }
        match time::sleep(milliseconds) {
            Ok(()) => {
                unsafe { out.write(0) };
                ABI_OK
            }
            Err(message) => unsafe { error(buffer, ABI_RUNTIME_ERROR, message) },
        }
    }

    macro_rules! int_binary {
        ($name:ident, $operation:ident) => {
            #[doc = "# Safety\n`out` and `buffer` must be writable, aligned, disjoint live slots. The caller must free any returned buffer exactly once with dever_rt_v1_buffer_free."]
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $name(
                left: i64,
                right: i64,
                out: *mut i64,
                buffer: *mut Buffer,
            ) -> u32 {
                // SAFETY: this export's pointer contract covers both out slots.
                unsafe { value(out, buffer, number::$operation(left, right)) }
            }
        };
    }

    int_binary!(dever_rt_v1_int_add, int_add);
    int_binary!(dever_rt_v1_int_sub, int_sub);
    int_binary!(dever_rt_v1_int_mul, int_mul);
    int_binary!(dever_rt_v1_int_div, int_div);
    int_binary!(dever_rt_v1_int_rem, int_rem);

    /// # Safety
    /// `out` and `buffer` must be writable, aligned, disjoint live slots.
    /// Free any returned buffer exactly once with dever_rt_v1_buffer_free.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_int_neg(
        input: i64,
        out: *mut i64,
        buffer: *mut Buffer,
    ) -> u32 {
        // SAFETY: this export's pointer contract covers both out slots.
        unsafe { value(out, buffer, number::int_neg(input)) }
    }

    macro_rules! float_binary {
        ($name:ident, $operation:tt) => {
            #[doc = "# Safety\n`out` and `buffer` must be writable, aligned, disjoint live slots. The caller must free any returned buffer exactly once with dever_rt_v1_buffer_free."]
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $name(
                left: f64,
                right: f64,
                out: *mut f64,
                buffer: *mut Buffer,
            ) -> u32 {
                // SAFETY: this export's pointer contract covers both out slots.
                unsafe { value(out, buffer, Ok(left $operation right)) }
            }
        };
    }

    float_binary!(dever_rt_v1_float_add, +);
    float_binary!(dever_rt_v1_float_sub, -);
    float_binary!(dever_rt_v1_float_mul, *);
    float_binary!(dever_rt_v1_float_div, /);
    float_binary!(dever_rt_v1_float_rem, %);

    /// # Safety
    /// `out` and `buffer` must be writable, aligned, disjoint live slots.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_float_neg(
        input: f64,
        out: *mut f64,
        buffer: *mut Buffer,
    ) -> u32 {
        // SAFETY: this export's pointer contract covers both out slots.
        unsafe { value(out, buffer, Ok(-input)) }
    }

    fn decimal_binary(
        left_low: u64,
        left_high: u64,
        right_low: u64,
        right_high: u64,
        operation: fn(
            number::DecimalValue,
            number::DecimalValue,
        ) -> Result<number::DecimalValue, &'static str>,
    ) -> Result<AbiDecimal, &'static str> {
        let left = AbiDecimal {
            low: left_low,
            high: left_high,
        }
        .decode()?;
        let right = AbiDecimal {
            low: right_low,
            high: right_high,
        }
        .decode()?;
        operation(left, right).map(AbiDecimal::encode)
    }

    macro_rules! decimal_binary {
        ($name:ident, $operation:ident) => {
            #[doc = "# Safety\n`out` and `buffer` must be writable, aligned, disjoint live slots. The caller must free any returned buffer exactly once with dever_rt_v1_buffer_free."]
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $name(
                left_low: u64,
                left_high: u64,
                right_low: u64,
                right_high: u64,
                out: *mut AbiDecimal,
                buffer: *mut Buffer,
            ) -> u32 {
                // SAFETY: this export's pointer contract covers both out slots.
                unsafe { value(out, buffer, decimal_binary(
                    left_low, left_high, right_low, right_high,
                    number::DecimalValue::$operation,
                )) }
            }
        };
    }

    decimal_binary!(dever_rt_v1_decimal_add, checked_add);
    decimal_binary!(dever_rt_v1_decimal_sub, checked_sub);
    decimal_binary!(dever_rt_v1_decimal_mul, checked_mul);
    decimal_binary!(dever_rt_v1_decimal_div, checked_div);

    /// # Safety
    /// `out` and `buffer` must be writable, aligned, disjoint live slots.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_decimal_from_int(
        input: i64,
        out: *mut AbiDecimal,
        buffer: *mut Buffer,
    ) -> u32 {
        // SAFETY: this export's pointer contract covers both out slots.
        unsafe {
            value(
                out,
                buffer,
                Ok(AbiDecimal::encode(number::DecimalValue::from_int(input))),
            )
        }
    }

    /// # Safety
    /// `out` and `buffer` must be writable, aligned, disjoint live slots.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_decimal_neg(
        low: u64,
        high: u64,
        out: *mut AbiDecimal,
        buffer: *mut Buffer,
    ) -> u32 {
        let result = AbiDecimal { low, high }
            .decode()
            .and_then(number::DecimalValue::checked_neg)
            .map(AbiDecimal::encode);
        // SAFETY: this export's pointer contract covers both out slots.
        unsafe { value(out, buffer, result) }
    }

    /// # Safety
    /// `out` and `buffer` must be writable, aligned, disjoint live slots.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_decimal_round(
        low: u64,
        high: u64,
        places: i64,
        out: *mut AbiDecimal,
        buffer: *mut Buffer,
    ) -> u32 {
        let result = AbiDecimal { low, high }
            .decode()
            .and_then(|value| value.round(places))
            .map(AbiDecimal::encode);
        // SAFETY: this export's pointer contract covers both out slots.
        unsafe { value(out, buffer, result) }
    }

    /// # Safety
    /// `buffer` must be a writable, aligned live slot. Free returned bytes once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_int_to_text(input: i64, buffer: *mut Buffer) -> u32 {
        // SAFETY: this export's pointer contract covers the Buffer slot.
        unsafe { text(buffer, Ok(abi::int_to_text(input).into_bytes())) }
    }

    /// # Safety
    /// `buffer` must be a writable, aligned live slot. Free returned bytes once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_float_to_text(input: f64, buffer: *mut Buffer) -> u32 {
        // SAFETY: this export's pointer contract covers the Buffer slot.
        unsafe { text(buffer, Ok(abi::float_to_text(input).into_bytes())) }
    }

    /// # Safety
    /// `buffer` must be a writable, aligned live slot. Free returned bytes once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_decimal_to_text(
        low: u64,
        high: u64,
        buffer: *mut Buffer,
    ) -> u32 {
        // SAFETY: this export's pointer contract covers the Buffer slot.
        unsafe {
            text(
                buffer,
                abi::decimal_to_text(AbiDecimal { low, high })
                    .map(String::into_bytes)
                    .map_err(str::to_owned),
            )
        }
    }

    unsafe fn copy_input(
        input_ptr: *const u8,
        input_len: u64,
        buffer: *mut Buffer,
        convert: impl FnOnce(&[u8]) -> Result<Vec<u8>, String>,
    ) -> u32 {
        // SAFETY: the exported caller promises a live Buffer slot.
        if !unsafe { empty(buffer) } {
            return ABI_INVALID_INPUT;
        }
        // SAFETY: exported callers promise the input remains live and unchanged
        // until this synchronous call returns; input() checks null/length first.
        let input = match unsafe { input(input_ptr, input_len) } {
            Ok(input) => input,
            // SAFETY: empty() validated and initialized the Buffer slot.
            Err(message) => return unsafe { error(buffer, ABI_INVALID_INPUT, message) },
        };
        match convert(input) {
            Ok(bytes) => {
                // SAFETY: empty() validated and initialized the Buffer slot.
                unsafe { owned_bytes(buffer, bytes) };
                ABI_OK
            }
            // SAFETY: empty() validated and initialized the Buffer slot.
            Err(message) => unsafe { error(buffer, ABI_RUNTIME_ERROR, message) },
        }
    }

    /// # Safety
    /// `input_ptr` must address `input_len` live, immutable bytes when nonzero;
    /// `buffer` must be writable, aligned, and disjoint from the input.
    /// Free returned bytes exactly once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_text_copy_utf8(
        input_ptr: *const u8,
        input_len: u64,
        buffer: *mut Buffer,
    ) -> u32 {
        // SAFETY: this export's pointer contract covers input and output.
        unsafe { copy_input(input_ptr, input_len, buffer, abi::text_from_utf8) }
    }

    /// # Safety
    /// `input_ptr` must address `input_len` live, immutable bytes when nonzero;
    /// `buffer` must be writable, aligned, and disjoint from the input.
    /// Free returned bytes exactly once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_bytes_copy(
        input_ptr: *const u8,
        input_len: u64,
        buffer: *mut Buffer,
    ) -> u32 {
        // SAFETY: this export's pointer contract covers input and output.
        unsafe { copy_input(input_ptr, input_len, buffer, |bytes| Ok(bytes.to_vec())) }
    }

    /// # Safety
    /// `input_ptr` must address `input_len` live, immutable bytes when nonzero;
    /// `buffer` must be writable, aligned, and disjoint from the input.
    /// Free returned bytes exactly once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_bytes_to_text(
        input_ptr: *const u8,
        input_len: u64,
        buffer: *mut Buffer,
    ) -> u32 {
        // SAFETY: this export's pointer contract covers input and output.
        unsafe { copy_input(input_ptr, input_len, buffer, abi::text_from_utf8) }
    }

    /// # Safety
    /// `ptr` and `len` must be one exact, still-owned pair returned by this
    /// runtime ABI, passed at most once; null with zero length is a no-op.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dever_rt_v1_buffer_free(ptr: *mut u8, len: u64) {
        if ptr.is_null() && len == 0 {
            return;
        }
        let Ok(len) = usize::try_from(len) else {
            return;
        };
        if ptr.is_null() || len == 0 {
            return;
        }
        // SAFETY: only owned_bytes creates nonempty buffers, using Box<[u8]>
        // from the same Rust allocator; the caller transfers that exact pair once.
        drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(ptr, len)) });
    }

    mod managed {
        use std::ffi::c_void;
        use std::hash::{Hash, Hasher};
        use std::mem::MaybeUninit;
        use std::ptr;
        use std::sync::Arc;

        use dever_runtime::bytes::Bytes;
        use dever_runtime::collections::{IntoValues, List, Map, MapEntry};
        use dever_runtime::{number, resource, text as text_runtime};

        use super::*;

        #[unsafe(no_mangle)]
        pub extern "C" fn dever_rt_v1_log_flush() {
            dever_runtime::log::flush();
        }

        // System ABI delegates domain validation and sensitive storage to runtime.
        macro_rules! system_call {
            ($name:ident($($arg:ident: $ty:ty),*) -> $out:ty, $complete:ident, $body:block) => {
                /// # Safety
                /// Handles borrow the exact canonical types; output/error are distinct writable slots.
                #[unsafe(no_mangle)]
                pub unsafe extern "C" fn $name($($arg:$ty,)* out: *mut $out, buffer: *mut Buffer) -> u32 {
                    if let Err(status) = unsafe { checked_output(out, buffer) } { return status; }
                    let execute = || $body;
                    let result: Result<_, AbiFailure> = execute();
                    unsafe { $complete(out, buffer, result) }
                }
            };
        }

        #[cfg(feature = "runtime-api")]
        mod api_context {
            use super::*;
            use dever_runtime::{api, auth};

            fn auth_error(error: auth::Error) -> AbiFailure {
                runtime_error(error.to_string())
            }
            unsafe fn text<'a>(value: *const c_void) -> Result<&'a str, AbiFailure> {
                unsafe { borrowed::<String>(value) }.map(String::as_str)
            }
            unsafe fn optional_text<'a>(
                value: *const c_void,
            ) -> Result<Option<&'a str>, AbiFailure> {
                if value.is_null() {
                    Ok(None)
                } else {
                    unsafe { text(value) }.map(Some)
                }
            }
            #[repr(C)]
            pub struct CookieOptions {
                path: *const c_void,
                domain: *const c_void,
                same_site: i64,
                secure: u8,
                http_only: u8,
                max_age: i64,
                max_age_present: u8,
            }
            unsafe fn cookie_options(
                raw: *const CookieOptions,
            ) -> Result<api::CookieOptions, AbiFailure> {
                let row = unsafe { borrowed::<CookieOptions>(raw.cast()) }?;
                if row.secure > 1 || row.http_only > 1 || row.max_age_present > 1 {
                    return Err(invalid("invalid Cookie boolean"));
                }
                Ok(api::CookieOptions {
                    path: unsafe { text(row.path) }?.to_owned(),
                    domain: unsafe { optional_text(row.domain) }?.map(str::to_owned),
                    same_site: match row.same_site {
                        0 => api::SameSite::Strict,
                        1 => api::SameSite::Lax,
                        2 => api::SameSite::None,
                        _ => return Err(invalid("invalid SameSite")),
                    },
                    secure: row.secure == 1,
                    http_only: row.http_only == 1,
                    max_age: (row.max_age_present == 1).then_some(row.max_age),
                })
            }
            system_call!(dever_rt_v1_api_request_id() -> *mut c_void, complete_handle, { api::request_id().map_err(runtime_error) });
            system_call!(dever_rt_v1_api_method() -> *mut c_void, complete_handle, { api::method().map_err(runtime_error) });
            system_call!(dever_rt_v1_api_path() -> *mut c_void, complete_handle, { api::path().map_err(runtime_error) });
            system_call!(dever_rt_v1_auth_site_key() -> *mut c_void, complete_handle, { auth::site_key().map_err(auth_error) });
            system_call!(dever_rt_v1_auth_id() -> *mut c_void, complete_handle, { auth::id().map_err(auth_error) });
            system_call!(dever_rt_v1_auth_session() -> *mut c_void, complete_handle, { auth::session().map_err(auth_error) });
            system_call!(dever_rt_v1_auth_owns_user(user: i64) -> u8, complete, { auth::owns_user(user).map(u8::from).map_err(auth_error) });
            system_call!(dever_rt_v1_auth_issue(subject: *const c_void, session: *const c_void, tenant: *const c_void) -> *mut c_void, complete_handle, { auth::issue(unsafe { text(subject) }?, unsafe { text(session) }?, unsafe { optional_text(tenant) }?).map_err(runtime_error) });
            system_call!(dever_rt_v1_auth_issue_cookie(subject: *const c_void, session: *const c_void, tenant: *const c_void) -> u8, complete, { auth::issue_cookie(unsafe { text(subject) }?, unsafe { text(session) }?, unsafe { optional_text(tenant) }?).map(|()| 0).map_err(runtime_error) });
            system_call!(dever_rt_v1_auth_clear_cookie() -> u8, complete, { auth::clear_cookie().map(|()| 0).map_err(runtime_error) });
            system_call!(dever_rt_v1_api_response_header(name: *const c_void, value: *const c_void) -> u8, complete, { api::response_header(unsafe { text(name) }?, unsafe { text(value) }?).map(|()| 0).map_err(runtime_error) });
            system_call!(dever_rt_v1_api_response_cookie(name: *const c_void, value: *const c_void, options: *const CookieOptions) -> u8, complete, { api::response_cookie(unsafe { text(name) }?, unsafe { text(value) }?, unsafe { cookie_options(options) }?).map(|()| 0).map_err(runtime_error) });
            system_call!(dever_rt_v1_api_response_secret_cookie(name: *const c_void, value: *const c_void, options: *const CookieOptions) -> u8, complete, { api::response_secret_cookie(unsafe { text(name) }?, unsafe { borrowed::<dever_runtime::secret::Secret>(value) }?.clone(), unsafe { cookie_options(options) }?).map(|()| 0).map_err(runtime_error) });

            macro_rules! optional {
                ($name:ident($($arg:ident:$ty:ty),*) -> $out:ty, $finish:ident, $body:block) => {
                    /// # Safety
                    /// Handles borrow canonical owners; outputs are distinct writable slots.
                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn $name($($arg:$ty,)* out:*mut $out, present:*mut u8, buffer:*mut Buffer) -> u32 {
                        if let Err(status) = unsafe { checked_optional_output(out, present, buffer) } { return status; }
                        let execute = || $body;
                        unsafe { $finish(out, present, buffer, execute()) }
                    }
                };
            }
            optional!(dever_rt_v1_api_client_address() -> *mut c_void, complete_optional_handle, { api::client_address().map_err(runtime_error) });
            optional!(dever_rt_v1_api_header(name:*const c_void) -> *mut c_void, complete_optional_handle, { api::header(unsafe { text(name) }?).map_err(runtime_error) });
            optional!(dever_rt_v1_api_cookie(name:*const c_void) -> *mut c_void, complete_optional_handle, { api::cookie(unsafe { text(name) }?).map_err(runtime_error) });
            optional!(dever_rt_v1_api_secret_cookie(name:*const c_void) -> *mut c_void, complete_optional_handle, { api::secret_cookie(unsafe { text(name) }?).map_err(runtime_error) });
            optional!(dever_rt_v1_auth_user_id() -> i64, complete_optional, { auth::user_id().map_err(auth_error) });
            optional!(dever_rt_v1_auth_tenant_id() -> i64, complete_optional, { auth::tenant_id().map_err(auth_error) });

            /// # Safety
            /// cause is an initialized nullable Text owner slot; message is borrowed.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_api_cause_append(
                cause: *mut *mut c_void,
                message: *const c_void,
            ) {
                unsafe { append_fault_cause(cause, message) };
            }
        }

        mod application {
            use super::*;
            use dever_runtime::api::Inputs;
            use dever_runtime::config::Settings;
            use dever_runtime::wire::{Encoder, Node};

            #[repr(C)]
            pub struct WireName {
                ptr: *const u8,
                len: u64,
            }

            type Decode = unsafe extern "C" fn(*const c_void, *mut c_void, *mut Buffer) -> u32;
            type CmdDecode = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut Buffer) -> u32;
            type Encode = unsafe extern "C" fn(*const c_void, *mut c_void, *mut Buffer) -> u32;

            // Node lifetimes are confined to wire_decode's synchronous callback.
            // This conversion never constructs an owner or extends the parsed tree.
            unsafe fn node<'a>(raw: *const c_void) -> Result<&'a Node<'a>, AbiFailure> {
                unsafe { borrowed(raw) }
            }

            unsafe fn node_text<'a>(raw: *const c_void) -> Result<&'a str, AbiFailure> {
                unsafe { node(raw) }?.text().map_err(runtime_error)
            }

            unsafe fn encoder<'a>(raw: *mut c_void) -> Result<&'a mut Encoder, AbiFailure> {
                if raw.is_null() || !raw.cast::<Encoder>().is_aligned() {
                    return Err(invalid("invalid wire encoder"));
                }
                // SAFETY: generated codecs borrow this encoder exclusively and
                // synchronously from wire_encode; no reference survives a call.
                Ok(unsafe { &mut *raw.cast::<Encoder>() })
            }

            unsafe fn name<'a>(raw: *const WireName) -> Result<&'a str, AbiFailure> {
                let name = unsafe { borrowed::<WireName>(raw.cast()) }?;
                let bytes = unsafe { input(name.ptr, name.len) }.map_err(invalid)?;
                std::str::from_utf8(bytes).map_err(|_| invalid("wire name is not UTF-8"))
            }

            unsafe fn names<'a>(
                raw: *const WireName,
                count: u64,
            ) -> Result<Vec<&'a str>, AbiFailure> {
                let count =
                    usize::try_from(count).map_err(|_| invalid("invalid wire name count"))?;
                if count > isize::MAX as usize / size_of::<WireName>()
                    || (count != 0 && (raw.is_null() || !raw.is_aligned()))
                {
                    return Err(invalid("invalid wire names"));
                }
                (0..count)
                    .map(|index| unsafe { name(raw.add(index)) })
                    .collect()
            }

            unsafe fn concrete_slot(
                ty: *const TypeDescriptor,
                slot: *const c_void,
            ) -> Result<ValueOps, AbiFailure> {
                let ty = unsafe { descriptor(ty, false) }?;
                if slot.is_null() || !(slot as usize).is_multiple_of(ty.align) {
                    return Err(invalid("invalid wire concrete value slot"));
                }
                Ok(ty)
            }

            unsafe fn status(buffer: *mut Buffer, result: Result<(), AbiFailure>) -> u32 {
                match result {
                    Ok(()) => ABI_OK,
                    Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
                }
            }

            unsafe fn callback_status(status: u32, buffer: *mut Buffer) -> u32 {
                // SAFETY: the callback follows the canonical Buffer ownership
                // contract even if it returns an unrecognized status code.
                let reply = unsafe { &*buffer };
                if matches!(status, ABI_RUNTIME_ERROR | ABI_INVALID_INPUT)
                    || (status == ABI_OK && reply.ptr.is_null() && reply.len == 0)
                {
                    return status;
                }
                unsafe { dever_rt_v1_buffer_free(reply.ptr, reply.len) };
                unsafe {
                    error(
                        buffer,
                        ABI_INVALID_INPUT,
                        "invalid wire codec callback result",
                    )
                }
            }

            unsafe fn finish_decoding(
                result: u32,
                operations: ValueOps,
                out: *mut c_void,
                buffer: *mut Buffer,
                finish: impl FnOnce() -> Result<(), AbiFailure>,
            ) -> u32 {
                let mut checked = unsafe { callback_status(result, buffer) };
                if result != ABI_OK {
                    return checked;
                }
                if checked == ABI_OK {
                    checked = unsafe { status(buffer, finish()) };
                }
                if checked != ABI_OK {
                    // The callback completed this concrete owner. A rejected
                    // Buffer or trailing CMD field must release it exactly once.
                    unsafe { (operations.drop)(out) };
                }
                checked
            }

            /// # Safety
            /// type and callback are process-static. out is an uninitialized
            /// concrete slot; callback obeys the scoped Node and cleanup contract.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_wire_decode(
                text: *const c_void,
                ty: *const TypeDescriptor,
                decode: Option<Decode>,
                out: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                if !unsafe { empty(buffer) } {
                    return ABI_INVALID_INPUT;
                }
                let execute = || -> Result<(u32, ValueOps), AbiFailure> {
                    let operations = unsafe { concrete_slot(ty, out) }?;
                    let decode = decode.ok_or_else(|| invalid("missing wire decoder"))?;
                    let text = unsafe { borrowed::<String>(text) }?;
                    let root = dever_runtime::wire::parse(text).map_err(runtime_error)?;
                    // SAFETY: root and all of its borrowed raw spans remain
                    // alive throughout this synchronous generated codec call.
                    Ok((
                        unsafe { decode(ptr::from_ref(&root).cast(), out, buffer) },
                        operations,
                    ))
                };
                match execute() {
                    Ok((result, operations)) => unsafe {
                        finish_decoding(result, operations, out, buffer, || Ok(()))
                    },
                    Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
                }
            }

            /// # Safety
            /// value has exactly type's layout; encode only borrows it and the
            /// scoped encoder. out and buffer are disjoint writable slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_wire_encode(
                value: *const c_void,
                ty: *const TypeDescriptor,
                encode: Option<Encode>,
                out: *mut *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(result) = unsafe { checked_output(out, buffer) } {
                    return result;
                }
                let prepare = || -> Result<Encode, AbiFailure> {
                    unsafe { concrete_slot(ty, value) }?;
                    encode.ok_or_else(|| invalid("missing wire encoder callback"))
                };
                let encode = match prepare() {
                    Ok(encode) => encode,
                    Err(failure) => {
                        return unsafe { error(buffer, failure.status, failure.message) };
                    }
                };
                let mut writer = Encoder::default();
                let result = unsafe { encode(value, ptr::from_mut(&mut writer).cast(), buffer) };
                let result = unsafe { callback_status(result, buffer) };
                if result != ABI_OK {
                    return result;
                }
                let encoded = writer
                    .finish()
                    .map(|encoded| encoded.as_str().to_owned())
                    .map_err(runtime_error);
                unsafe { complete_handle(out, buffer, encoded) }
            }

            /// # Safety
            /// type is process-static. The callback borrows Inputs exclusively
            /// during this call and cleans partial outputs on callback failure.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_cmd_decode(
                text: *const c_void,
                ty: *const TypeDescriptor,
                decode: Option<CmdDecode>,
                out: *mut c_void,
                buffer: *mut Buffer,
            ) -> u32 {
                if !unsafe { empty(buffer) } {
                    return ABI_INVALID_INPUT;
                }
                let execute = || -> Result<(u32, ValueOps, Inputs), AbiFailure> {
                    let operations = unsafe { concrete_slot(ty, out) }?;
                    let decode = decode.ok_or_else(|| invalid("missing CMD decoder"))?;
                    let text = unsafe { borrowed::<String>(text) }?;
                    let mut inputs = Inputs::from_json_object(text)
                        .map_err(|failure| runtime_error(failure.to_string()))?;
                    let result = unsafe { decode(ptr::from_mut(&mut inputs).cast(), out, buffer) };
                    Ok((result, operations, inputs))
                };
                match execute() {
                    Ok((result, operations, inputs)) => unsafe {
                        finish_decoding(result, operations, out, buffer, || {
                            inputs
                                .finish()
                                .map_err(|failure| runtime_error(failure.to_string()))
                        })
                    },
                    Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
                }
            }

            /// # Safety
            /// inputs is an exclusive borrow from cmd_decode; name is borrowed
            /// UTF-8 and outputs are distinct writable canonical slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_cmd_input_field(
                inputs: *mut c_void,
                field: *const WireName,
                required: u8,
                out: *mut *mut c_void,
                present: *mut u8,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(result) = unsafe { checked_output(out, buffer) } {
                    return result;
                }
                if !slot_valid(present) || required > 1 {
                    return unsafe {
                        error(
                            buffer,
                            ABI_INVALID_INPUT,
                            "invalid CMD input output or requirement",
                        )
                    };
                }
                let execute = || -> Result<Option<String>, AbiFailure> {
                    if inputs.is_null() || !inputs.cast::<Inputs>().is_aligned() {
                        return Err(invalid("invalid CMD inputs"));
                    }
                    let field = unsafe { name(field) }?;
                    // SAFETY: cmd_decode owns this Inputs and grants the
                    // generated synchronous callback its only mutable borrow.
                    let inputs = unsafe { &mut *inputs.cast::<Inputs>() };
                    if required != 0 {
                        inputs.raw_json(field).map(Some)
                    } else {
                        inputs.optional_raw_json(field)
                    }
                    .map_err(|failure| runtime_error(failure.to_string()))
                };
                unsafe { complete_optional_handle(out, present, buffer, execute()) }
            }

            system_call!(dever_rt_v1_wire_node_is_null(raw: *const c_void) -> u8, complete, {
                Ok(u8::from(unsafe { node(raw) }?.is_null()))
            });
            system_call!(dever_rt_v1_wire_node_bool(raw: *const c_void) -> u8, complete, {
                unsafe { node(raw) }?.boolean().map(u8::from).map_err(runtime_error)
            });
            system_call!(dever_rt_v1_wire_node_int(raw: *const c_void) -> i64, complete, {
                unsafe { node(raw) }?.int().map_err(runtime_error)
            });
            system_call!(dever_rt_v1_wire_node_float(raw: *const c_void) -> f64, complete, {
                unsafe { node(raw) }?.float().map_err(runtime_error)
            });
            system_call!(dever_rt_v1_wire_node_text(raw: *const c_void) -> *mut c_void, complete_handle, {
                Ok(unsafe { node_text(raw) }?.to_owned())
            });
            system_call!(dever_rt_v1_wire_node_bytes(raw: *const c_void) -> *mut c_void, complete_handle, {
                unsafe { node(raw) }?.bytes().map_err(runtime_error)
            });
            system_call!(dever_rt_v1_wire_node_raw(raw: *const c_void) -> *mut c_void, complete_handle, {
                Ok(unsafe { node(raw) }?.raw().to_owned())
            });
            system_call!(dever_rt_v1_wire_node_decimal(raw: *const c_void) -> AbiDecimal, complete, {
                number::DecimalValue::parse(unsafe { node_text(raw) }?).map(AbiDecimal::encode).map_err(runtime_error)
            });
            system_call!(dever_rt_v1_wire_node_uuid(raw: *const c_void) -> *mut c_void, complete_handle, {
                dever_runtime::orm::Uuid::parse(unsafe { node_text(raw) }?).map_err(|failure| runtime_error(failure.to_string()))
            });
            system_call!(dever_rt_v1_wire_node_secret(raw: *const c_void) -> *mut c_void, complete_handle, {
                Ok(dever_runtime::secret::Secret::from_input(unsafe { node_text(raw) }?.as_bytes().to_vec()))
            });
            system_call!(dever_rt_v1_wire_node_datetime(raw: *const c_void) -> i64, complete, {
                time::parse_datetime(unsafe { node_text(raw) }?).map_err(runtime_error)
            });
            system_call!(dever_rt_v1_wire_node_date(raw: *const c_void) -> i64, complete, {
                time::parse_date(unsafe { node_text(raw) }?).map_err(runtime_error)
            });
            system_call!(dever_rt_v1_wire_node_time(raw: *const c_void) -> i64, complete, {
                time::parse_time(unsafe { node_text(raw) }?).map_err(runtime_error)
            });
            system_call!(dever_rt_v1_wire_node_list_len(raw: *const c_void) -> i64, complete, {
                Ok(unsafe { node(raw) }?.list().map_err(runtime_error)?.len() as i64)
            });
            system_call!(dever_rt_v1_wire_node_list_at(raw: *const c_void, index: i64) -> *const c_void, complete, {
                let values = unsafe { node(raw) }?.list().map_err(runtime_error)?;
                let index = usize::try_from(index).map_err(|_| invalid("invalid wire List index"))?;
                values.get(index).map(|value| ptr::from_ref(value).cast()).ok_or_else(|| invalid("invalid wire List index"))
            });

            macro_rules! wire_operation {
                ($name:ident($($arg:ident: $ty:ty),*) $body:block) => {
                    /// # Safety
                    /// All wire pointers are scoped borrows from the current
                    /// codec callback; buffer is a writable canonical error slot.
                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn $name($($arg: $ty,)* buffer: *mut Buffer) -> u32 {
                        if !unsafe { empty(buffer) } { return ABI_INVALID_INPUT; }
                        let execute = || -> Result<(), AbiFailure> { $body };
                        unsafe { status(buffer, execute()) }
                    }
                };
            }

            wire_operation!(dever_rt_v1_wire_node_fields(raw: *const c_void, allowed: *const WireName, count: u64) {
                let allowed = unsafe { names(allowed, count) }?;
                unsafe { node(raw) }?.fields(&allowed).map(|_| ()).map_err(runtime_error)
            });

            wire_operation!(dever_rt_v1_job_decode_unit(payload: *const c_void) {
                let payload = unsafe { borrowed::<String>(payload) }?;
                let value = dever_runtime::wire::parse(payload).map_err(runtime_error)?;
                if !value.is_null() { return Err(runtime_error("expected null Job payload")); }
                Ok(())
            });

            /// # Safety
            /// Node/name are borrowed from this codec call. out, present and
            /// buffer are distinct writable slots; out is initialized if present.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_wire_node_field(
                raw: *const c_void,
                field: *const WireName,
                required: u8,
                out: *mut *const c_void,
                present: *mut u8,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(result) = unsafe { checked_output(out, buffer) } {
                    return result;
                }
                if !slot_valid(present) || required > 1 {
                    return unsafe {
                        error(
                            buffer,
                            ABI_INVALID_INPUT,
                            "invalid wire field output or requirement",
                        )
                    };
                }
                let execute = || -> Result<Option<*const c_void>, AbiFailure> {
                    let field = unsafe { name(field) }?;
                    let value = unsafe { node(raw) }?
                        .object()
                        .map_err(runtime_error)?
                        .get(field);
                    if value.is_none() && required != 0 {
                        return Err(runtime_error("missing required wire field"));
                    }
                    Ok(value.map(|value| ptr::from_ref(value).cast()))
                };
                unsafe { complete_optional(out, present, buffer, execute()) }
            }

            wire_operation!(dever_rt_v1_wire_encoder_null(raw: *mut c_void) {
                unsafe { encoder(raw) }?.null().map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_bool(raw: *mut c_void, value: u8) {
                if value > 1 { return Err(invalid("invalid wire Bool")); }
                unsafe { encoder(raw) }?.boolean(value != 0).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_int(raw: *mut c_void, value: i64) {
                unsafe { encoder(raw) }?.int(value).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_float(raw: *mut c_void, value: f64) {
                unsafe { encoder(raw) }?.float(value).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_text(raw: *mut c_void, text: *const c_void) {
                let value = unsafe { borrowed::<String>(text) }?;
                unsafe { encoder(raw) }?.text(value).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_bytes(raw: *mut c_void, bytes: *const c_void) {
                let value = unsafe { borrowed::<Bytes>(bytes) }?;
                unsafe { encoder(raw) }?.bytes(value).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_json(raw: *mut c_void, text: *const c_void) {
                let value = unsafe { borrowed::<String>(text) }?;
                unsafe { encoder(raw) }?.json(value).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_decimal(raw: *mut c_void, low: u64, high: u64) {
                let value = AbiDecimal { low, high }.decode().map_err(runtime_error)?;
                unsafe { encoder(raw) }?.text(&value.to_string()).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_uuid(raw: *mut c_void, uuid: *const c_void) {
                let value = unsafe { borrowed::<dever_runtime::orm::Uuid>(uuid) }?;
                unsafe { encoder(raw) }?.text(&value.to_string()).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_datetime(raw: *mut c_void, value: i64) {
                let text = time::format_datetime(value).map_err(runtime_error)?;
                unsafe { encoder(raw) }?.text(&text).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_date(raw: *mut c_void, value: i64) {
                let text = time::format_date(value).map_err(runtime_error)?;
                unsafe { encoder(raw) }?.text(&text).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_time(raw: *mut c_void, value: i64) {
                let text = time::format_time(value).map_err(runtime_error)?;
                unsafe { encoder(raw) }?.text(&text).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_begin_array(raw: *mut c_void) {
                unsafe { encoder(raw) }?.begin_array().map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_begin_object(raw: *mut c_void) {
                unsafe { encoder(raw) }?.begin_object().map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_key(raw: *mut c_void, field: *const WireName) {
                let field = unsafe { name(field) }?;
                unsafe { encoder(raw) }?.key(field).map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_wire_encoder_end(raw: *mut c_void) {
                unsafe { encoder(raw) }?.end().map_err(runtime_error)
            });

            system_call!(dever_rt_v1_adapter_settings_load() -> *mut c_void, complete_handle, {
                dever_runtime::component::adapter_settings().map_err(runtime_error)
            });
            wire_operation!(dever_rt_v1_adapter_settings_validate_names(raw: *const c_void, identities: *const WireName, count: u64) {
                let identities = unsafe { names(identities, count) }?;
                unsafe { borrowed::<Arc<Settings>>(raw) }?.validate_adapter_names(&identities).map_err(runtime_error)
            });

            /// # Safety
            /// settings is a live owner, names are borrowed UTF-8, and every
            /// output is distinct, writable and correctly aligned.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_adapter_settings_select(
                raw: *const c_void,
                identity: *const WireName,
                candidates: *const WireName,
                count: u64,
                index: *mut i64,
                setting: *mut *mut c_void,
                present: *mut u8,
                buffer: *mut Buffer,
            ) -> u32 {
                if let Err(result) = unsafe { checked_output(index, buffer) } {
                    return result;
                }
                if !slot_valid(setting) || !slot_valid(present) {
                    return unsafe {
                        error(
                            buffer,
                            ABI_INVALID_INPUT,
                            "invalid Adapter selection output",
                        )
                    };
                }
                let execute = || -> Result<(), AbiFailure> {
                    let identity = unsafe { name(identity) }?;
                    let candidates = unsafe { names(candidates, count) }?;
                    let (selected, value) = unsafe { borrowed::<Arc<Settings>>(raw) }?
                        .select_adapter(identity, &candidates)
                        .map_err(runtime_error)?;
                    unsafe {
                        index.write(selected as i64);
                        present.write(u8::from(value.is_some()));
                        if let Some(value) = value {
                            setting.write(owned(value));
                        }
                    }
                    Ok(())
                };
                unsafe { status(buffer, execute()) }
            }

            /// # Safety
            /// settings is null or one live owner returned by settings_load.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn dever_rt_v1_adapter_settings_release(settings: *const c_void) {
                unsafe { release::<Arc<Settings>>(settings.cast_mut()) };
            }
        }

        system_call!(dever_rt_v1_job_clock_new(test: u8) -> *mut c_void, complete_handle, {
            match test {
                0 => Ok(time::Clock::default()),
                1 => Ok(time::Clock::for_test()),
                _ => Err(invalid("invalid Job clock mode")),
            }
        });
        system_call!(dever_rt_v1_job_clock_now(clock: *const c_void) -> i64, complete, {
            unsafe { borrowed::<time::Clock>(clock) }?.now().map_err(|error| runtime_error(error.to_string()))
        });
        /// # Safety
        /// Borrows one Clock. Buffer is a distinct writable diagnostic slot.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_job_clock_advance(
            clock: *const c_void,
            millis: i64,
            buffer: *mut Buffer,
        ) -> u32 {
            if !unsafe { empty(buffer) } {
                return ABI_INVALID_INPUT;
            }
            let result = unsafe { borrowed::<time::Clock>(clock) }
                .and_then(|clock| clock.advance(millis).map_err(runtime_error));
            match result {
                Ok(()) => 0,
                Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
            }
        }
        /// # Safety
        /// Transfers one live Clock handle, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_job_clock_release(clock: *mut c_void) {
            unsafe { release::<time::Clock>(clock) };
        }

        system_call!(dever_rt_v1_time_unix_millis() -> i64, complete, { time::unix_millis().map_err(runtime_error) });
        system_call!(dever_rt_v1_time_now() -> i64, complete, { time::now().map_err(runtime_error) });
        system_call!(dever_rt_v1_time_monotonic_nanos() -> i64, complete, { time::monotonic_nanos().map_err(runtime_error) });
        system_call!(dever_rt_v1_time_parse_datetime(text: *const c_void) -> i64, complete, { time::parse_datetime(unsafe { borrowed::<String>(text) }?).map_err(runtime_error) });
        system_call!(dever_rt_v1_time_format_datetime(value: i64) -> *mut c_void, complete_handle, { time::format_datetime(value).map_err(runtime_error) });
        system_call!(dever_rt_v1_time_parse_date(text: *const c_void) -> i64, complete, { time::parse_date(unsafe { borrowed::<String>(text) }?).map_err(runtime_error) });
        system_call!(dever_rt_v1_time_format_date(value: i64) -> *mut c_void, complete_handle, { time::format_date(value).map_err(runtime_error) });
        system_call!(dever_rt_v1_time_parse_time(text: *const c_void) -> i64, complete, { time::parse_time(unsafe { borrowed::<String>(text) }?).map_err(runtime_error) });
        system_call!(dever_rt_v1_time_format_time(value: i64) -> *mut c_void, complete_handle, { time::format_time(value).map_err(runtime_error) });
        system_call!(dever_rt_v1_time_add(left: i64, right: i64) -> i64, complete, { time::add(left, right).map_err(runtime_error) });
        system_call!(dever_rt_v1_time_subtract(left: i64, right: i64) -> i64, complete, { time::subtract(left, right).map_err(runtime_error) });
        system_call!(dever_rt_v1_time_difference(left: i64, right: i64) -> i64, complete, { time::difference(left, right).map_err(runtime_error) });

        system_call!(dever_rt_v1_crypto_token(length: i64) -> *mut c_void, complete_handle, { dever_runtime::crypto::token(length).map_err(runtime_error) });
        system_call!(dever_rt_v1_crypto_password_hash(password: *const c_void) -> *mut c_void, complete_handle, { dever_runtime::crypto::password_hash(unsafe { borrowed(password) }?).map_err(runtime_error) });
        system_call!(dever_rt_v1_crypto_password_verify(password: *const c_void, encoded: *const c_void) -> u8, complete, { dever_runtime::crypto::password_verify(unsafe { borrowed(password) }?, unsafe { borrowed::<String>(encoded) }?).map(u8::from).map_err(runtime_error) });
        system_call!(dever_rt_v1_crypto_sha256(bytes: *const c_void) -> *mut c_void, complete_handle, { Ok(dever_runtime::crypto::sha256(unsafe { borrowed(bytes) }?)) });
        system_call!(dever_rt_v1_crypto_hmac_sha256(secret: *const c_void, bytes: *const c_void) -> *mut c_void, complete_handle, { Ok(dever_runtime::crypto::hmac_sha256(unsafe { borrowed(secret) }?, unsafe { borrowed(bytes) }?)) });
        system_call!(dever_rt_v1_crypto_constant_time_eq(left: *const c_void, right: *const c_void) -> u8, complete, { Ok(u8::from(dever_runtime::crypto::constant_time_eq(unsafe { borrowed(left) }?, unsafe { borrowed(right) }?))) });
        system_call!(dever_rt_v1_secret_from_text(text: *const c_void) -> *mut c_void, complete_handle, { Ok(dever_runtime::secret::Secret::from_input(unsafe { borrowed::<String>(text) }?.as_bytes().to_vec())) });
        system_call!(dever_rt_v1_uuid_to_text(uuid: *const c_void) -> *mut c_void, complete_handle, { Ok(unsafe { borrowed::<dever_runtime::orm::Uuid>(uuid) }?.to_string()) });
        system_call!(dever_rt_v1_stdout_write(text: *const c_void) -> u8, complete, { resource::stdout_write(unsafe { borrowed::<String>(text) }?).map(|()| 0).map_err(runtime_error) });
        system_call!(dever_rt_v1_log_write(level: u32, message: *const c_void, fields: *const c_void) -> u8, complete, {
            let level = match level { 0 => dever_runtime::log::Level::Debug, 1 => dever_runtime::log::Level::Info, 2 => dever_runtime::log::Level::Warn, 3 => dever_runtime::log::Level::Error, _ => return Err(invalid("invalid log level")) };
            let message = unsafe { borrowed::<String>(message) }?.clone();
            let fields = unsafe { borrowed::<MapHandle>(fields) }?;
            if fields.key.size != size_of::<*const c_void>() || fields.value.size != size_of::<*const c_void>() { return Err(invalid("log fields require Text values")); }
            let fields = fields.values.pairs().map(|(name, value)| {
                let name = unsafe { name.0.pointer().cast::<*const c_void>().read() };
                let value = unsafe { value.pointer().cast::<*const c_void>().read() };
                Ok(dever_runtime::log::Field { name: unsafe { borrowed::<String>(name) }?.clone(), value: unsafe { borrowed::<String>(value) }?.clone() })
            }).collect::<Result<Vec<_>, AbiFailure>>()?;
            dever_runtime::log::emit(level, message, fields);
            Ok(0)
        });
        system_call!(dever_rt_v1_process_arguments(ty: *const TypeDescriptor) -> *mut c_void, complete_handle, {
            let element = unsafe { descriptor(ty, false) }?;
            if element.size != size_of::<*const c_void>() || element.align != align_of::<*const c_void>() { return Err(invalid("process arguments require Text descriptor")); }
            let arguments = dever_runtime::process::arguments().map_err(runtime_error)?;
            let mut values = Vec::with_capacity(arguments.values().len());
            for argument in arguments.into_values() {
                let text = owned(argument);
                let value = unsafe { Element::from_source(element, std::ptr::from_ref(&text).cast()) };
                unsafe { release::<String>(text) };
                values.push(value?);
            }
            Ok(ListHandle { element, values: List::new(values) })
        });

        /// # Safety
        /// `text` borrows a live Text owner until return; it is not consumed.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_process_error(text: *const c_void) -> u32 {
            let Ok(message) = (unsafe { borrowed::<String>(text) }) else {
                return ABI_INVALID_INPUT;
            };
            dever_runtime::log::error(message.clone(), Vec::new());
            ABI_OK
        }

        /// # Safety
        /// text is borrowed; output/present/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_uuid_parse(
            text: *const c_void,
            out: *mut *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { borrowed::<String>(text) }
                .map(|text| dever_runtime::orm::Uuid::parse(text).ok());
            unsafe { complete_optional_handle(out, present, buffer, result) }
        }
        /// # Safety
        /// Both handles borrow canonical UUID owners.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_uuid_equal(
            left: *const c_void,
            right: *const c_void,
        ) -> u8 {
            match (
                unsafe { borrowed::<dever_runtime::orm::Uuid>(left) },
                unsafe { borrowed::<dever_runtime::orm::Uuid>(right) },
            ) {
                (Ok(left), Ok(right)) => u8::from(left == right),
                _ => 0,
            }
        }
        /// # Safety
        /// handle borrows the matching opaque owner.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_secret_retain(handle: *const c_void) -> *mut c_void {
            unsafe { retain::<dever_runtime::secret::Secret>(handle) }
        }
        /// # Safety
        /// handle transfers one matching opaque owner, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_secret_release(handle: *mut c_void) {
            unsafe { release::<dever_runtime::secret::Secret>(handle) }
        }

        /// # Safety
        /// handle borrows the matching opaque owner.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_uuid_retain(handle: *const c_void) -> *mut c_void {
            unsafe { retain::<dever_runtime::orm::Uuid>(handle) }
        }
        /// # Safety
        /// handle transfers one matching opaque owner, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_uuid_release(handle: *mut c_void) {
            unsafe { release::<dever_runtime::orm::Uuid>(handle) }
        }

        type CloneValue = unsafe extern "C" fn(*const c_void, *mut c_void);
        type DropValue = unsafe extern "C" fn(*mut c_void);
        type EqualValue = unsafe extern "C" fn(*const c_void, *const c_void) -> u8;
        type HashValue = unsafe extern "C" fn(*const c_void) -> u64;

        #[repr(C)]
        pub struct TypeDescriptor {
            size: u64,
            align: u64,
            clone: Option<CloneValue>,
            drop: Option<DropValue>,
            equal: Option<EqualValue>,
            hash: Option<HashValue>,
        }

        #[repr(C)]
        pub struct ReadEventDescriptor {
            pub(super) element_type: *const TypeDescriptor,
            pub(super) chunk: Option<unsafe extern "C" fn(*const c_void, *mut c_void)>,
            pub(super) failed: Option<unsafe extern "C" fn(*const c_void, *mut c_void)>,
        }

        #[derive(Clone, Copy)]
        pub(super) struct ValueOps {
            identity: usize,
            pub(super) size: usize,
            words: usize,
            pub(super) align: usize,
            clone: CloneValue,
            drop: DropValue,
            equal: EqualValue,
            hash: Option<HashValue>,
        }

        #[derive(Debug)]
        pub(super) struct AbiFailure {
            pub(super) status: u32,
            pub(super) message: String,
        }

        pub(super) fn invalid(message: impl Into<String>) -> AbiFailure {
            AbiFailure {
                status: ABI_INVALID_INPUT,
                message: message.into(),
            }
        }

        pub(super) fn runtime_error(message: impl Into<String>) -> AbiFailure {
            AbiFailure {
                status: ABI_RUNTIME_ERROR,
                message: message.into(),
            }
        }

        pub(super) unsafe fn descriptor(
            raw: *const TypeDescriptor,
            key: bool,
        ) -> Result<ValueOps, AbiFailure> {
            if raw.is_null() || !raw.is_aligned() {
                return Err(invalid("invalid ABI type descriptor"));
            }
            // SAFETY: the private compiler supplies a live, aligned, process-static descriptor.
            let value = unsafe { &*raw };
            let size =
                usize::try_from(value.size).map_err(|_| invalid("invalid ABI element size"))?;
            let align = usize::try_from(value.align)
                .map_err(|_| invalid("invalid ABI element alignment"))?;
            if size > isize::MAX as usize || !align.is_power_of_two() || align > align_of::<u64>() {
                return Err(invalid("invalid ABI element layout"));
            }
            let words = size
                .checked_add(7)
                .ok_or_else(|| invalid("invalid ABI element size"))?
                / 8;
            let (Some(clone), Some(drop), Some(equal)) = (value.clone, value.drop, value.equal)
            else {
                return Err(invalid("incomplete ABI type descriptor"));
            };
            if key && value.hash.is_none() {
                return Err(invalid("Map key descriptor requires hash"));
            }
            Ok(ValueOps {
                identity: raw as usize,
                size,
                words: words.max(1),
                align,
                clone,
                drop,
                equal,
                hash: value.hash,
            })
        }

        fn slot_valid<T>(slot: *mut T) -> bool {
            !slot.is_null() && slot.is_aligned()
        }

        pub(super) unsafe fn complete<T: Copy>(
            out: *mut T,
            buffer: *mut Buffer,
            result: Result<T, AbiFailure>,
        ) -> u32 {
            // SAFETY: each export promises the Buffer slot is live and writable.
            if !unsafe { empty(buffer) } {
                return ABI_INVALID_INPUT;
            }
            if !slot_valid(out) {
                // SAFETY: empty initialized this valid Buffer slot.
                return unsafe { error(buffer, ABI_INVALID_INPUT, "invalid ABI output pointer") };
            }
            match result {
                Ok(value) => {
                    // SAFETY: the caller promises a disjoint, writable, aligned output slot.
                    unsafe { out.write(value) };
                    ABI_OK
                }
                // SAFETY: empty initialized this valid Buffer slot.
                Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
            }
        }

        unsafe fn complete_optional<T: Copy>(
            out: *mut T,
            present: *mut u8,
            buffer: *mut Buffer,
            result: Result<Option<T>, AbiFailure>,
        ) -> u32 {
            // SAFETY: the caller promises a live Buffer slot.
            if !unsafe { empty(buffer) } {
                return ABI_INVALID_INPUT;
            }
            if !slot_valid(out) || !slot_valid(present) {
                // SAFETY: empty initialized this valid Buffer slot.
                return unsafe { error(buffer, ABI_INVALID_INPUT, "invalid ABI output pointer") };
            }
            match result {
                Ok(value) => {
                    // SAFETY: out and present are distinct writable slots by the export contract.
                    unsafe { present.write(u8::from(value.is_some())) };
                    if let Some(value) = value {
                        // SAFETY: a present value initializes the previously uninitialized slot.
                        unsafe { out.write(value) };
                    }
                    ABI_OK
                }
                // SAFETY: empty initialized this valid Buffer slot.
                Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
            }
        }

        pub(super) unsafe fn complete_handle<T>(
            out: *mut *mut c_void,
            buffer: *mut Buffer,
            result: Result<T, AbiFailure>,
        ) -> u32 {
            // SAFETY: every managed export promises a live Buffer slot.
            if !unsafe { empty(buffer) } {
                return ABI_INVALID_INPUT;
            }
            if !slot_valid(out) {
                // SAFETY: empty initialized this valid Buffer slot.
                return unsafe { error(buffer, ABI_INVALID_INPUT, "invalid ABI output pointer") };
            }
            match result {
                Ok(value) => {
                    // SAFETY: out is a distinct, live pointer slot.
                    unsafe { out.write(owned(value)) };
                    ABI_OK
                }
                // SAFETY: empty initialized this valid Buffer slot.
                Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
            }
        }

        unsafe fn complete_optional_handle<T>(
            out: *mut *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
            result: Result<Option<T>, AbiFailure>,
        ) -> u32 {
            // SAFETY: every managed export promises a live Buffer slot.
            if !unsafe { empty(buffer) } {
                return ABI_INVALID_INPUT;
            }
            if !slot_valid(out) || !slot_valid(present) {
                // SAFETY: empty initialized this valid Buffer slot.
                return unsafe { error(buffer, ABI_INVALID_INPUT, "invalid ABI output pointer") };
            }
            match result {
                Ok(value) => {
                    // SAFETY: the two output slots are distinct and writable.
                    unsafe { present.write(u8::from(value.is_some())) };
                    if let Some(value) = value {
                        unsafe { out.write(owned(value)) };
                    }
                    ABI_OK
                }
                // SAFETY: empty initialized this valid Buffer slot.
                Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
            }
        }

        pub(super) fn owned<T>(value: T) -> *mut c_void {
            Arc::into_raw(Arc::new(value)) as *mut c_void
        }

        pub(super) unsafe fn append_fault_cause(cause: *mut *mut c_void, message: *const c_void) {
            if cause.is_null() || !cause.is_aligned() {
                return;
            }
            let Ok(message) = (unsafe { borrowed::<String>(message) }) else {
                return;
            };
            let previous = unsafe { *cause };
            let text = if previous.is_null() {
                format!("caused by: {message}")
            } else {
                let Ok(previous) = (unsafe { borrowed::<String>(previous) }) else {
                    return;
                };
                format!("{previous}; caused by: {message}")
            };
            unsafe {
                release::<String>(previous);
                cause.write(owned(text));
            }
        }

        pub(super) unsafe fn borrowed<'a, T>(handle: *const c_void) -> Result<&'a T, AbiFailure> {
            if handle.is_null() || !(handle as *const T).is_aligned() {
                return Err(invalid("invalid ABI handle"));
            }
            // SAFETY: the private caller guarantees a live handle of this exact owner type.
            Ok(unsafe { &*(handle as *const T) })
        }

        pub(super) unsafe fn consumed<T>(handle: *mut c_void) -> Result<Arc<T>, AbiFailure> {
            if handle.is_null() || !(handle as *const T).is_aligned() {
                return Err(invalid("invalid ABI handle"));
            }
            // SAFETY: each _take export consumes exactly one Arc owner from into_raw.
            Ok(unsafe { Arc::from_raw(handle as *const T) })
        }

        pub(super) unsafe fn retain<T>(handle: *const c_void) -> *mut c_void {
            if handle.is_null() {
                return ptr::null_mut();
            }
            // SAFETY: the caller guarantees this is a live Arc<T> owner. Incrementing
            // creates one independent owner represented by the returned same pointer.
            unsafe { Arc::increment_strong_count(handle as *const T) };
            handle as *mut c_void
        }

        pub(super) unsafe fn release<T>(handle: *mut c_void) {
            if !handle.is_null() {
                // SAFETY: the caller transfers one live Arc<T> owner exactly once.
                drop(unsafe { Arc::from_raw(handle as *const T) });
            }
        }

        pub(super) struct Element {
            words: Box<[MaybeUninit<u64>]>,
            ops: ValueOps,
        }

        impl Element {
            pub(super) fn from_initializer(
                ops: ValueOps,
                initialize: impl FnOnce(*mut c_void),
            ) -> Self {
                let mut words = vec![MaybeUninit::new(0u64); ops.words].into_boxed_slice();
                initialize(words.as_mut_ptr().cast());
                Self { words, ops }
            }

            pub(super) unsafe fn from_source(
                ops: ValueOps,
                source: *const c_void,
            ) -> Result<Self, AbiFailure> {
                if source.is_null() || !(source as usize).is_multiple_of(ops.align) {
                    return Err(invalid("invalid ABI element pointer"));
                }
                let mut words = vec![MaybeUninit::new(0u64); ops.words].into_boxed_slice();
                // SAFETY: source is a live initialized concrete value and words have
                // enough target-layout bytes at at least the required alignment.
                unsafe { (ops.clone)(source, words.as_mut_ptr().cast()) };
                Ok(Self { words, ops })
            }

            pub(super) fn pointer(&self) -> *const c_void {
                self.words.as_ptr().cast()
            }

            pub(super) unsafe fn copy_to(&self, out: *mut c_void) -> Result<(), AbiFailure> {
                if out.is_null() || !(out as usize).is_multiple_of(self.ops.align) {
                    return Err(invalid("invalid ABI element output"));
                }
                // SAFETY: the caller supplies an uninitialized, aligned destination
                // for this exact type; callback creates its independent owner.
                unsafe { (self.ops.clone)(self.pointer(), out) };
                Ok(())
            }
        }

        impl Clone for Element {
            fn clone(&self) -> Self {
                let mut words = vec![MaybeUninit::new(0u64); self.ops.words].into_boxed_slice();
                // SAFETY: both pointers refer to this descriptor's exact type.
                unsafe { (self.ops.clone)(self.pointer(), words.as_mut_ptr().cast()) };
                Self {
                    words,
                    ops: self.ops,
                }
            }
        }

        impl Drop for Element {
            fn drop(&mut self) {
                // SAFETY: the callback initialized this concrete value exactly once.
                unsafe { (self.ops.drop)(self.words.as_mut_ptr().cast()) };
            }
        }

        impl PartialEq for Element {
            fn eq(&self, other: &Self) -> bool {
                self.ops.identity == other.ops.identity &&
                    // SAFETY: matching descriptors guarantee both values have the same type.
                    unsafe { (self.ops.equal)(self.pointer(), other.pointer()) != 0 }
            }
        }

        #[derive(Clone)]
        struct MapKey(Element);

        impl PartialEq for MapKey {
            fn eq(&self, other: &Self) -> bool {
                self.0 == other.0
            }
        }
        // The checker permits only reflexive, hashable key types. General
        // elements remain PartialEq because Float NaN is not equal to itself.
        impl Eq for MapKey {}

        impl Hash for MapKey {
            fn hash<H: Hasher>(&self, state: &mut H) {
                let hash = self.0.ops.hash.expect("validated Map key descriptor");
                // SAFETY: Map keys are fully initialized values of this descriptor.
                state.write_u64(unsafe { hash(self.0.pointer()) });
            }
        }

        #[derive(Clone)]
        pub(super) struct ListHandle {
            pub(super) values: List<Element>,
            pub(super) element: ValueOps,
        }
        #[derive(Clone)]
        struct MapHandle {
            values: Map<MapKey, Element>,
            key: ValueOps,
            value: ValueOps,
        }

        struct ListCursor {
            values: IntoValues<Element>,
        }
        struct MapCursor {
            values: IntoValues<MapEntry<MapKey, Element>>,
            key: ValueOps,
            value: ValueOps,
        }

        pub(super) struct StreamHandle {
            pub(super) values: resource::Stream<Element>,
            element: ValueOps,
        }

        pub(super) fn initialize_event<T>(
            source: T,
            element: ValueOps,
            callback: unsafe extern "C" fn(*const c_void, *mut c_void),
        ) -> Element {
            let handle = owned(source);
            let value = Element::from_initializer(element, |out| {
                // SAFETY: the process-static callback initializes one complete event
                // from this borrowed handle and retains any stored reference.
                unsafe { callback(handle, out) };
            });
            // SAFETY: owned created exactly one temporary Arc<T> owner.
            unsafe { release::<T>(handle) };
            value
        }

        fn take_unique<T: Clone>(value: Arc<T>) -> T {
            Arc::try_unwrap(value).unwrap_or_else(|shared| (*shared).clone())
        }

        unsafe fn inputs<'a>(
            values: *const *const c_void,
            count: u64,
        ) -> Result<&'a [*const c_void], AbiFailure> {
            let count = usize::try_from(count).map_err(|_| invalid("invalid ABI element count"))?;
            if count == 0 {
                return Ok(&[]);
            }
            if values.is_null()
                || !values.is_aligned()
                || count > (isize::MAX as usize / size_of::<*const c_void>())
            {
                return Err(invalid("invalid ABI element array"));
            }
            // SAFETY: the private caller guarantees count live pointer slots.
            Ok(unsafe { std::slice::from_raw_parts(values, count) })
        }

        unsafe fn write_element_optional(
            value: impl FnOnce() -> Result<Option<Element>, AbiFailure>,
            out: *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            // SAFETY: each export promises a live Buffer slot.
            if !unsafe { empty(buffer) } {
                return ABI_INVALID_INPUT;
            }
            if !slot_valid(present) || out.is_null() {
                // SAFETY: empty initialized this valid Buffer slot.
                return unsafe { error(buffer, ABI_INVALID_INPUT, "invalid ABI output pointer") };
            }
            match value() {
                Ok(Some(element)) => {
                    // SAFETY: the caller promises disjoint output slots, including
                    // storage large and aligned enough for the concrete element.
                    if let Err(failure) = unsafe { element.copy_to(out) } {
                        return unsafe { error(buffer, failure.status, failure.message) };
                    }
                    unsafe { present.write(1) };
                    ABI_OK
                }
                Ok(None) => {
                    unsafe { present.write(0) };
                    ABI_OK
                }
                // SAFETY: empty initialized this valid Buffer slot.
                Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
            }
        }

        macro_rules! handle_owner {
            ($retain:ident, $release:ident, $ty:ty) => {
                /// # Safety
                /// A non-null handle must be one live owner of this exact managed type.
                /// The returned handle owns one additional reference; null is a no-op.
                #[unsafe(no_mangle)]
                pub unsafe extern "C" fn $retain(handle: *const c_void) -> *mut c_void {
                    // SAFETY: this export's private handle contract establishes type and lifetime.
                    unsafe { retain::<$ty>(handle) }
                }

                /// # Safety
                /// A non-null handle must be one owned reference of this exact type,
                /// transferred exactly once; null is a no-op.
                #[unsafe(no_mangle)]
                pub unsafe extern "C" fn $release(handle: *mut c_void) {
                    // SAFETY: this export consumes the caller's one reference.
                    unsafe { release::<$ty>(handle) }
                }
            };
        }

        handle_owner!(dever_rt_v1_text_retain, dever_rt_v1_text_release, String);
        handle_owner!(dever_rt_v1_bytes_retain, dever_rt_v1_bytes_release, Bytes);
        handle_owner!(
            dever_rt_v1_list_retain,
            dever_rt_v1_list_release,
            ListHandle
        );
        handle_owner!(dever_rt_v1_map_retain, dever_rt_v1_map_release, MapHandle);
        handle_owner!(
            dever_rt_v1_file_retain,
            dever_rt_v1_file_release,
            resource::FileHandle
        );
        handle_owner!(
            dever_rt_v1_stream_retain,
            dever_rt_v1_stream_release,
            StreamHandle
        );
        handle_owner!(
            dever_rt_v1_socket_retain,
            dever_rt_v1_socket_release,
            dever_runtime::net::Socket
        );
        handle_owner!(
            dever_rt_v1_listener_retain,
            dever_rt_v1_listener_release,
            dever_runtime::net::Listener
        );

        /// # Safety
        /// listener is borrowed; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_net_port(
            listener: *const c_void,
            out: *mut i64,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_output(out, buffer) } {
                return status;
            }
            let result = unsafe { borrowed::<dever_runtime::net::Listener>(listener) }
                .and_then(|listener| dever_runtime::net::port(listener).map_err(runtime_error));
            unsafe { complete(out, buffer, result) }
        }

        /// # Safety
        /// socket is borrowed; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_net_timeout(
            socket: *const c_void,
            milliseconds: i64,
            out: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_output(out, buffer) } {
                return status;
            }
            let result =
                unsafe { borrowed::<dever_runtime::net::Socket>(socket) }.and_then(|socket| {
                    dever_runtime::net::timeout(socket, milliseconds)
                        .map(|()| 1)
                        .map_err(runtime_error)
                });
            unsafe { complete(out, buffer, result) }
        }

        /// # Safety
        /// socket is borrowed; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_socket_close(
            socket: *const c_void,
            out: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_output(out, buffer) } {
                return status;
            }
            let result = unsafe { borrowed::<dever_runtime::net::Socket>(socket) }
                .and_then(|socket| socket.close().map(|()| 1).map_err(runtime_error));
            unsafe { complete(out, buffer, result) }
        }

        /// # Safety
        /// listener is borrowed; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_listener_close(
            listener: *const c_void,
            out: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_output(out, buffer) } {
                return status;
            }
            let result = unsafe { borrowed::<dever_runtime::net::Listener>(listener) }
                .and_then(|listener| listener.close().map(|()| 1).map_err(runtime_error));
            unsafe { complete(out, buffer, result) }
        }

        pub(super) unsafe fn checked_output<T>(
            out: *mut T,
            buffer: *mut Buffer,
        ) -> Result<(), u32> {
            if !unsafe { empty(buffer) } {
                return Err(ABI_INVALID_INPUT);
            }
            if !slot_valid(out) {
                return Err(unsafe {
                    error(buffer, ABI_INVALID_INPUT, "invalid ABI output pointer")
                });
            }
            Ok(())
        }

        unsafe fn checked_optional_output<T>(
            out: *mut T,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> Result<(), u32> {
            unsafe { checked_output(out, buffer) }?;
            if !slot_valid(present) {
                return Err(unsafe {
                    error(buffer, ABI_INVALID_INPUT, "invalid ABI output pointer")
                });
            }
            Ok(())
        }

        /// # Safety
        /// path is a live borrowed Text handle. Output slots are distinct and writable.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_file_open(
            path: *const c_void,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_output(out, buffer) } {
                return status;
            }
            let result = unsafe { borrowed::<String>(path) }
                .and_then(|path| resource::file_open(path).map_err(runtime_error));
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// path is a live borrowed Text handle. Output slots are distinct and writable.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_file_create(
            path: *const c_void,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_output(out, buffer) } {
                return status;
            }
            let result = unsafe { borrowed::<String>(path) }
                .and_then(|path| resource::file_create(path).map_err(runtime_error));
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// file is a live borrowed File; out/present/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_file_read(
            file: *const c_void,
            limit: i64,
            out: *mut *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_optional_output(out, present, buffer) } {
                return status;
            }
            let result = unsafe { borrowed::<resource::FileHandle>(file) }
                .and_then(|file| resource::read(file, limit).map_err(runtime_error));
            unsafe { complete_optional_handle(out, present, buffer, result) }
        }

        /// # Safety
        /// file and bytes are live borrowed handles; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_file_write(
            file: *const c_void,
            bytes: *const c_void,
            out: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_output(out, buffer) } {
                return status;
            }
            let result = (|| {
                let file = unsafe { borrowed::<resource::FileHandle>(file) }?;
                let bytes = unsafe { borrowed::<Bytes>(bytes) }?;
                resource::write(file, bytes).map_err(runtime_error)?;
                Ok(1)
            })();
            unsafe { complete(out, buffer, result) }
        }

        /// # Safety
        /// file is a live borrowed File; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_file_close(
            file: *const c_void,
            out: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_output(out, buffer) } {
                return status;
            }
            let result = unsafe { borrowed::<resource::FileHandle>(file) }
                .and_then(|file| file.close().map(|()| 1).map_err(runtime_error));
            unsafe { complete(out, buffer, result) }
        }

        /// # Safety
        /// file is a live borrowed File. Event callbacks and type descriptor are
        /// process-static and initialize complete values without unwinding.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_file_chunks(
            file: *const c_void,
            limit: i64,
            event: *const ReadEventDescriptor,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_output(out, buffer) } {
                return status;
            }
            let result = (|| {
                let file = unsafe { borrowed::<resource::FileHandle>(file) }?;
                if event.is_null() || !event.is_aligned() {
                    return Err(invalid("invalid ABI read event descriptor"));
                }
                let event = unsafe { &*event };
                let element = unsafe { descriptor(event.element_type, false) }?;
                let (Some(chunk), Some(failed)) = (event.chunk, event.failed) else {
                    return Err(invalid("incomplete ABI read event descriptor"));
                };
                let values = resource::read_chunks(file.clone(), limit)
                    .map_err(runtime_error)?
                    .map(move |result| match result {
                        Ok(bytes) => initialize_event(bytes, element, chunk),
                        Err(message) => initialize_event(message, element, failed),
                    });
                Ok(StreamHandle { values, element })
            })();
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// stream is a live borrowed Stream; out/present/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_stream_pull(
            stream: *const c_void,
            out: *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_optional_output(out, present, buffer) } {
                return status;
            }
            let stream = match unsafe { borrowed::<StreamHandle>(stream) } {
                Ok(stream) => stream,
                Err(failure) => return unsafe { error(buffer, failure.status, failure.message) },
            };
            if !(out as usize).is_multiple_of(stream.element.align) {
                return unsafe { error(buffer, ABI_INVALID_INPUT, "invalid ABI element output") };
            }
            match stream.values.pull() {
                Some(element) => {
                    if let Err(failure) = unsafe { element.copy_to(out) } {
                        return unsafe { error(buffer, failure.status, failure.message) };
                    }
                    unsafe { present.write(1) };
                }
                None => unsafe { present.write(0) },
            }
            ABI_OK
        }

        /// # Safety
        /// stream is a live borrowed Stream; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_stream_close(
            stream: *const c_void,
            out: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            if let Err(status) = unsafe { checked_output(out, buffer) } {
                return status;
            }
            let result = unsafe { borrowed::<StreamHandle>(stream) }.map(|stream| {
                stream.values.close();
                1
            });
            unsafe { complete(out, buffer, result) }
        }

        /// # Safety
        /// Input is borrowed for len bytes (null only for zero length); out and
        /// error are distinct writable slots. The returned handle is owned.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_new(
            input_ptr: *const u8,
            len: u64,
            out: *mut *mut c_void,
            error_buffer: *mut Buffer,
        ) -> u32 {
            // SAFETY: the caller keeps input live and immutable through this call.
            let result = unsafe { input(input_ptr, len) }
                .map_err(invalid)
                .and_then(|bytes| {
                    Bytes::new(bytes.to_vec())
                        .into_text()
                        .map_err(runtime_error)
                });
            // SAFETY: the caller promises distinct aligned writable output slots.
            unsafe { complete_handle(out, error_buffer, result) }
        }

        /// # Safety
        /// handle is a live borrowed Text handle; buffer is a distinct writable slot.
        /// Free non-null output through dever_rt_v1_buffer_free.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_export(
            handle: *const c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            // SAFETY: the private caller supplies a live Text handle.
            let result =
                unsafe { borrowed::<String>(handle) }.map(|value| value.as_bytes().to_vec());
            unsafe { export_bytes(buffer, result) }
        }

        unsafe fn export_bytes(buffer: *mut Buffer, result: Result<Vec<u8>, AbiFailure>) -> u32 {
            // SAFETY: the caller supplies a live output Buffer slot.
            if !unsafe { empty(buffer) } {
                return ABI_INVALID_INPUT;
            }
            match result {
                Ok(bytes) => {
                    unsafe { owned_bytes(buffer, bytes) };
                    ABI_OK
                }
                Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
            }
        }

        /// # Safety
        /// Both non-null handles are borrowed live Text handles.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_equal(
            left: *const c_void,
            right: *const c_void,
        ) -> u8 {
            // SAFETY: the caller guarantees the exact handle type and lifetime.
            u8::from(
                unsafe { borrowed::<String>(left).expect("valid Text handle") }
                    == unsafe { borrowed::<String>(right).expect("valid Text handle") },
            )
        }

        /// # Safety
        /// handle is a non-null live borrowed Text handle.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_hash(handle: *const c_void) -> u64 {
            let value = unsafe { borrowed::<String>(handle).expect("valid Text handle") };
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            value.hash(&mut hasher);
            hasher.finish()
        }

        /// # Safety
        /// Both handles are live borrowed Text; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_compare(
            left: *const c_void,
            right: *const c_void,
            out: *mut i32,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                let left = unsafe { borrowed::<String>(left) }?;
                let right = unsafe { borrowed::<String>(right) }?;
                Ok(match left.cmp(right) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                })
            })();
            unsafe { complete(out, buffer, result) }
        }

        /// # Safety
        /// Both handles are live borrowed Text; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_concat(
            left: *const c_void,
            right: *const c_void,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                let mut result = unsafe { borrowed::<String>(left) }?.clone();
                result.push_str(unsafe { borrowed::<String>(right) }?);
                Ok(result)
            })();
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// handle is live borrowed Text; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_length(
            handle: *const c_void,
            out: *mut i64,
            buffer: *mut Buffer,
        ) -> u32 {
            let result =
                unsafe { borrowed::<String>(handle) }.map(|value| value.chars().count() as i64);
            unsafe { complete(out, buffer, result) }
        }

        macro_rules! text_transform {
            ($name:ident, $body:expr) => {
                /// # Safety
                /// handle is live borrowed Text; out/error are distinct writable slots.
                #[unsafe(no_mangle)]
                pub unsafe extern "C" fn $name(
                    handle: *const c_void,
                    out: *mut *mut c_void,
                    buffer: *mut Buffer,
                ) -> u32 {
                    let result = unsafe { borrowed::<String>(handle) }.map($body);
                    unsafe { complete_handle(out, buffer, result) }
                }
            };
        }
        text_transform!(dever_rt_v1_text_trim, |value: &String| value
            .trim()
            .to_owned());
        text_transform!(dever_rt_v1_text_lower, |value: &String| value
            .to_lowercase());
        text_transform!(dever_rt_v1_text_upper, |value: &String| value
            .to_uppercase());

        /// # Safety
        /// handle is live borrowed Text; out/present/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_codepoint(
            handle: *const c_void,
            out: *mut i64,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            let result =
                unsafe { borrowed::<String>(handle) }.map(|value| text_runtime::codepoint(value));
            unsafe { complete_optional(out, present, buffer, result) }
        }

        /// # Safety
        /// out/present/error are distinct writable slots; returned handle is owned.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_from_codepoint(
            value: i64,
            out: *mut *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            unsafe {
                complete_optional_handle(
                    out,
                    present,
                    buffer,
                    Ok(text_runtime::from_codepoint(value)),
                )
            }
        }

        /// # Safety
        /// handle is live borrowed Text; out/present/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_at(
            handle: *const c_void,
            index: i64,
            out: *mut *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            let result =
                unsafe { borrowed::<String>(handle) }.map(|value| text_runtime::at(value, index));
            unsafe { complete_optional_handle(out, present, buffer, result) }
        }

        /// # Safety
        /// handle is live borrowed Text; out/present/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_slice(
            handle: *const c_void,
            start: i64,
            end: i64,
            out: *mut *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { borrowed::<String>(handle) }
                .map(|value| text_runtime::slice(value, start, end));
            unsafe { complete_optional_handle(out, present, buffer, result) }
        }

        /// # Safety
        /// handles are live borrowed Text; out/present/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_index_of(
            handle: *const c_void,
            pattern: *const c_void,
            out: *mut i64,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                Ok(text_runtime::index_of(
                    unsafe { borrowed::<String>(handle) }?,
                    unsafe { borrowed::<String>(pattern) }?,
                ))
            })();
            unsafe { complete_optional(out, present, buffer, result) }
        }

        macro_rules! text_predicate {
            ($name:ident, $method:ident) => {
                /// # Safety
                /// handles are live borrowed Text; out/error are distinct writable slots.
                #[unsafe(no_mangle)]
                pub unsafe extern "C" fn $name(
                    handle: *const c_void,
                    pattern: *const c_void,
                    out: *mut u8,
                    buffer: *mut Buffer,
                ) -> u32 {
                    let result = (|| {
                        Ok(u8::from(
                            unsafe { borrowed::<String>(handle) }?
                                .$method(unsafe { borrowed::<String>(pattern) }?),
                        ))
                    })();
                    unsafe { complete(out, buffer, result) }
                }
            };
        }
        text_predicate!(dever_rt_v1_text_contains, contains);
        text_predicate!(dever_rt_v1_text_starts_with, starts_with);
        text_predicate!(dever_rt_v1_text_ends_with, ends_with);

        /// # Safety
        /// handles are live borrowed Text; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_replace(
            handle: *const c_void,
            pattern: *const c_void,
            replacement: *const c_void,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                Ok(unsafe { borrowed::<String>(handle) }?
                    .replace(unsafe { borrowed::<String>(pattern) }?, unsafe {
                        borrowed::<String>(replacement)
                    }?))
            })();
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// text is live borrowed Text; out/present/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_int_parse(
            text: *const c_void,
            out: *mut i64,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { borrowed::<String>(text) }.map(|value| number::parse_int(value));
            unsafe { complete_optional(out, present, buffer, result) }
        }

        /// # Safety
        /// text is live borrowed Text; out/present/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_float_parse(
            text: *const c_void,
            out: *mut f64,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            let result =
                unsafe { borrowed::<String>(text) }.map(|value| number::parse_float(value));
            unsafe { complete_optional(out, present, buffer, result) }
        }

        /// # Safety
        /// text is live borrowed Text; out/present/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_decimal_parse(
            text: *const c_void,
            out: *mut AbiDecimal,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { borrowed::<String>(text) }
                .map(|value| number::parse_decimal(value).map(AbiDecimal::encode));
            unsafe { complete_optional(out, present, buffer, result) }
        }

        /// # Safety
        /// out/error are distinct writable slots; input words must be finite Decimal values.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_decimal_compare(
            left_low: u64,
            left_high: u64,
            right_low: u64,
            right_high: u64,
            out: *mut i32,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                let left = AbiDecimal {
                    low: left_low,
                    high: left_high,
                }
                .decode()
                .map_err(runtime_error)?;
                let right = AbiDecimal {
                    low: right_low,
                    high: right_high,
                }
                .decode()
                .map_err(runtime_error)?;
                Ok(match left.cmp(&right) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                })
            })();
            unsafe { complete(out, buffer, result) }
        }

        /// # Safety
        /// Both word pairs must encode compiler-checked finite Decimal values.
        /// This infallible typed callback helper compares numeric values, not bits.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_decimal_equal(
            left_low: u64,
            left_high: u64,
            right_low: u64,
            right_high: u64,
        ) -> u8 {
            let left = AbiDecimal {
                low: left_low,
                high: left_high,
            }
            .decode()
            .expect("checked finite Decimal");
            let right = AbiDecimal {
                low: right_low,
                high: right_high,
            }
            .decode()
            .expect("checked finite Decimal");
            u8::from(left == right)
        }

        /// # Safety
        /// Both handles are live borrowed Text; text_type describes the pointer
        /// representation of Text and remains live; out/error are writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_text_split(
            handle: *const c_void,
            separator: *const c_void,
            text_type: *const TypeDescriptor,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                let value = unsafe { borrowed::<String>(handle) }?;
                let separator = unsafe { borrowed::<String>(separator) }?;
                let ops = unsafe { descriptor(text_type, false) }?;
                if ops.size != size_of::<*mut c_void>() {
                    return Err(invalid("invalid Text element layout"));
                }
                let mut values = Vec::new();
                for piece in text_runtime::split(value, separator).into_values() {
                    let handle = owned(piece);
                    // SAFETY: this stack slot contains a live Text handle of the
                    // compiler-declared element type until the clone finishes.
                    let element = unsafe {
                        Element::from_source(ops, (&handle as *const *mut c_void).cast())
                    };
                    unsafe { release::<String>(handle) };
                    values.push(element?);
                }
                Ok(ListHandle {
                    values: List::new(values),
                    element: ops,
                })
            })();
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// Input is borrowed for len bytes (null only for zero); out/error are
        /// distinct writable slots. The returned Bytes handle is owned.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_bytes_new(
            input_ptr: *const u8,
            len: u64,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { input(input_ptr, len) }
                .map(|bytes| Bytes::new(bytes.to_vec()))
                .map_err(invalid);
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// handle is live borrowed Bytes; buffer is a distinct writable slot.
        /// Free non-null output via dever_rt_v1_buffer_free.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_bytes_export(
            handle: *const c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { borrowed::<Bytes>(handle) }.map(|value| value.values().to_vec());
            unsafe { export_bytes(buffer, result) }
        }

        /// # Safety
        /// Both handles are live borrowed Bytes handles.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_bytes_equal(
            left: *const c_void,
            right: *const c_void,
        ) -> u8 {
            u8::from(
                unsafe { borrowed::<Bytes>(left).expect("valid Bytes handle") }
                    == unsafe { borrowed::<Bytes>(right).expect("valid Bytes handle") },
            )
        }

        /// # Safety
        /// handle is live borrowed Bytes; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_bytes_length(
            handle: *const c_void,
            out: *mut i64,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { borrowed::<Bytes>(handle) }.map(Bytes::length);
            unsafe { complete(out, buffer, result) }
        }

        /// # Safety
        /// handle is live borrowed Bytes; out/present/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_bytes_at(
            handle: *const c_void,
            index: i64,
            out: *mut i64,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { borrowed::<Bytes>(handle) }.map(|value| value.at(index));
            unsafe { complete_optional(out, present, buffer, result) }
        }

        /// # Safety
        /// Both handles are live borrowed Bytes; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_bytes_concat(
            left: *const c_void,
            right: *const c_void,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                Ok(unsafe { borrowed::<Bytes>(left) }?
                    .clone()
                    .concat(unsafe { borrowed::<Bytes>(right) }?))
            })();
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// handle is live borrowed Bytes; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_bytes_slice(
            handle: *const c_void,
            start: i64,
            end: i64,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { borrowed::<Bytes>(handle) }
                .and_then(|value| value.slice(start, end).map_err(runtime_error));
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// text is live borrowed Text; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_bytes_from_text(
            text: *const c_void,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { borrowed::<String>(text) }.map(|value| Bytes::from_text(value));
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// bytes is live borrowed Bytes; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_bytes_to_text_handle(
            bytes: *const c_void,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { borrowed::<Bytes>(bytes) }
                .and_then(|value| value.to_text().map_err(runtime_error));
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// list is a live borrowed List<Int> handle; out/error are distinct
        /// writable slots. The caller's concrete descriptor has eight-byte Ints.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_bytes_from_ints(
            list: *const c_void,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                let list = unsafe { borrowed::<ListHandle>(list) }?;
                if list.element.size != size_of::<i64>() {
                    return Err(invalid("invalid Int element layout"));
                }
                let values = list
                    .values
                    .values()
                    .iter()
                    .map(|value| {
                        // SAFETY: the checked Bytes.fromInts call supplies List<Int>;
                        // Int is exactly one initialized, aligned i64 with no padding.
                        unsafe { (value.pointer() as *const i64).read() }
                    })
                    .collect::<Vec<_>>();
                Bytes::from_ints(&values).map_err(runtime_error)
            })();
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// element_type is a process-static descriptor. values points to count
        /// live borrowed element pointers (or is null when count is zero).
        /// out/error are distinct writable slots; returned handle is owned.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_list_new(
            element_type: *const TypeDescriptor,
            values: *const *const c_void,
            count: u64,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                let element = unsafe { descriptor(element_type, false) }?;
                let sources = unsafe { inputs(values, count) }?;
                let mut collected = Vec::with_capacity(sources.len());
                for source in sources {
                    collected.push(unsafe { Element::from_source(element, *source) }?);
                }
                Ok(ListHandle {
                    values: List::new(collected),
                    element,
                })
            })();
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// list transfers one live owned List handle even on failure; element is
        /// borrowed initialized storage of its descriptor's type. out/error are
        /// distinct writable slots and a successful out owns the new List.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_list_append_take(
            list: *mut c_void,
            element: *const c_void,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                let list = unsafe { consumed::<ListHandle>(list) }?;
                let mut list = take_unique(list);
                let element = unsafe { Element::from_source(list.element, element) }?;
                list.values = list.values.append(element);
                Ok(list)
            })();
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// list transfers one live owned List handle even on failure. out_element
        /// is uninitialized storage for its concrete type, present/error are
        /// distinct writable slots; only a present result initializes the value.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_list_first_take(
            list: *mut c_void,
            out_element: *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            let list = unsafe { consumed::<ListHandle>(list) };
            unsafe {
                write_element_optional(
                    || list.map(|list| take_unique(list).values.into_first()),
                    out_element,
                    present,
                    buffer,
                )
            }
        }

        /// # Safety
        /// list is a live borrowed List handle; out/error are writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_list_length(
            list: *const c_void,
            out: *mut i64,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { borrowed::<ListHandle>(list) }
                .map(|list| list.values.values().len() as i64);
            unsafe { complete(out, buffer, result) }
        }

        /// # Safety
        /// Both handles are live borrowed Lists of the same concrete element type.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_list_equal(
            left: *const c_void,
            right: *const c_void,
        ) -> u8 {
            let left = unsafe { borrowed::<ListHandle>(left).expect("valid List handle") };
            let right = unsafe { borrowed::<ListHandle>(right).expect("valid List handle") };
            u8::from(left.element.identity == right.element.identity && left.values == right.values)
        }

        /// # Safety
        /// list transfers one live owned List handle even on failure. out_cursor
        /// and error are distinct writable slots; returned cursor is owned.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_list_cursor_take(
            list: *mut c_void,
            out_cursor: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { consumed::<ListHandle>(list) }.map(|list| ListCursor {
                values: take_unique(list).values.into_values(),
            });
            // SAFETY: cursor is a Box owner, not an Arc managed value.
            unsafe { complete_cursor(out_cursor, buffer, result) }
        }

        unsafe fn complete_cursor<T>(
            out: *mut *mut c_void,
            buffer: *mut Buffer,
            result: Result<T, AbiFailure>,
        ) -> u32 {
            // SAFETY: every cursor export promises a live Buffer slot.
            if !unsafe { empty(buffer) } {
                return ABI_INVALID_INPUT;
            }
            if !slot_valid(out) {
                return unsafe { error(buffer, ABI_INVALID_INPUT, "invalid ABI output pointer") };
            }
            match result {
                Ok(cursor) => {
                    // SAFETY: out is a distinct writable pointer slot.
                    unsafe { out.write(Box::into_raw(Box::new(cursor)).cast()) };
                    ABI_OK
                }
                Err(failure) => unsafe { error(buffer, failure.status, failure.message) },
            }
        }

        unsafe fn cursor_mut<'a, T>(cursor: *mut c_void) -> Result<&'a mut T, AbiFailure> {
            if cursor.is_null() || !(cursor as *mut T).is_aligned() {
                return Err(invalid("invalid ABI cursor"));
            }
            // SAFETY: the caller grants this call exclusive access to a live cursor.
            Ok(unsafe { &mut *(cursor as *mut T) })
        }

        /// # Safety
        /// cursor is a live exclusive List cursor. out_element is uninitialized
        /// aligned storage for its element; present/error are distinct slots.
        /// A present element is independently owned and must be dropped once.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_list_cursor_next(
            cursor: *mut c_void,
            out_element: *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            unsafe {
                write_element_optional(
                    || cursor_mut::<ListCursor>(cursor).map(|cursor| cursor.values.next()),
                    out_element,
                    present,
                    buffer,
                )
            }
        }

        /// # Safety
        /// cursor is one owned List cursor returned by list_cursor_take, consumed
        /// exactly once; null is a no-op. Remaining values are destroyed.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_list_cursor_release(cursor: *mut c_void) {
            if !cursor.is_null() {
                // SAFETY: the caller transfers one Box<ListCursor> owner.
                drop(unsafe { Box::from_raw(cursor as *mut ListCursor) });
            }
        }

        /// # Safety
        /// Descriptors are process-static; keys/values point to count borrowed
        /// initialized concrete values (null arrays only with count zero).
        /// out/error are distinct writable slots and returned Map is owned.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_map_new(
            key_type: *const TypeDescriptor,
            value_type: *const TypeDescriptor,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: u64,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                let key = unsafe { descriptor(key_type, true) }?;
                let value = unsafe { descriptor(value_type, false) }?;
                let keys = unsafe { inputs(keys, count) }?;
                let values = unsafe { inputs(values, count) }?;
                let mut entries = Vec::with_capacity(keys.len());
                for (key_source, value_source) in keys.iter().zip(values) {
                    entries.push((
                        MapKey(unsafe { Element::from_source(key, *key_source) }?),
                        unsafe { Element::from_source(value, *value_source) }?,
                    ));
                }
                let values = Map::new(entries).map_err(runtime_error)?;
                Ok(MapHandle { values, key, value })
            })();
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// map transfers one live owned Map even on failure. key/value are
        /// borrowed initialized values of the stored descriptors; out/error are
        /// distinct writable slots. Returned Map is an owned COW value.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_map_put_take(
            map: *mut c_void,
            key: *const c_void,
            value: *const c_void,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                let map = unsafe { consumed::<MapHandle>(map) }?;
                let mut map = take_unique(map);
                let key = MapKey(unsafe { Element::from_source(map.key, key) }?);
                let value = unsafe { Element::from_source(map.value, value) }?;
                map.values = map.values.put(key, value);
                Ok(map)
            })();
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// map transfers one live owned Map even on failure. key is a borrowed
        /// initialized key value; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_map_remove_take(
            map: *mut c_void,
            key: *const c_void,
            out: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                let map = unsafe { consumed::<MapHandle>(map) }?;
                let mut map = take_unique(map);
                let key = MapKey(unsafe { Element::from_source(map.key, key) }?);
                map.values = map.values.remove(&key);
                Ok(map)
            })();
            unsafe { complete_handle(out, buffer, result) }
        }

        /// # Safety
        /// map transfers one live owned Map even on failure. key is borrowed;
        /// out_value is uninitialized storage of the concrete value type.
        /// present/error are distinct writable slots; present output is owned.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_map_get_take(
            map: *mut c_void,
            key: *const c_void,
            out_value: *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = (|| {
                let map = unsafe { consumed::<MapHandle>(map) }?;
                let map = take_unique(map);
                let key = MapKey(unsafe { Element::from_source(map.key, key) }?);
                Ok(map.values.into_get(&key))
            })();
            unsafe { write_element_optional(|| result, out_value, present, buffer) }
        }

        /// # Safety
        /// map is a live borrowed Map; out/error are distinct writable slots.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_map_length(
            map: *const c_void,
            out: *mut i64,
            buffer: *mut Buffer,
        ) -> u32 {
            let result =
                unsafe { borrowed::<MapHandle>(map) }.map(|map| map.values.pairs().count() as i64);
            unsafe { complete(out, buffer, result) }
        }

        /// # Safety
        /// Both handles are live borrowed Maps of the same key/value types.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_map_equal(
            left: *const c_void,
            right: *const c_void,
        ) -> u8 {
            let left = unsafe { borrowed::<MapHandle>(left).expect("valid Map handle") };
            let right = unsafe { borrowed::<MapHandle>(right).expect("valid Map handle") };
            u8::from(
                left.key.identity == right.key.identity
                    && left.value.identity == right.value.identity
                    && left.values == right.values,
            )
        }

        /// # Safety
        /// map transfers one live owned Map even on failure; out_cursor/error
        /// are distinct writable slots. Returned cursor is owned.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_map_cursor_take(
            map: *mut c_void,
            out_cursor: *mut *mut c_void,
            buffer: *mut Buffer,
        ) -> u32 {
            let result = unsafe { consumed::<MapHandle>(map) }.map(|map| {
                let map = take_unique(map);
                MapCursor {
                    values: map.values.into_entries().into_values(),
                    key: map.key,
                    value: map.value,
                }
            });
            unsafe { complete_cursor(out_cursor, buffer, result) }
        }

        /// # Safety
        /// cursor is a live exclusive Map cursor. out_key/out_value are distinct
        /// uninitialized concrete value slots, also distinct from present/error.
        /// Present outputs each own one independent value and must be dropped.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_map_cursor_next(
            cursor: *mut c_void,
            out_key: *mut c_void,
            out_value: *mut c_void,
            present: *mut u8,
            buffer: *mut Buffer,
        ) -> u32 {
            if !unsafe { empty(buffer) } {
                return ABI_INVALID_INPUT;
            }
            if !slot_valid(present) || out_key.is_null() || out_value.is_null() {
                return unsafe { error(buffer, ABI_INVALID_INPUT, "invalid ABI output pointer") };
            }
            let cursor = match unsafe { cursor_mut::<MapCursor>(cursor) } {
                Ok(cursor) => cursor,
                Err(failure) => return unsafe { error(buffer, failure.status, failure.message) },
            };
            if !(out_key as usize).is_multiple_of(cursor.key.align)
                || !(out_value as usize).is_multiple_of(cursor.value.align)
            {
                return unsafe { error(buffer, ABI_INVALID_INPUT, "invalid ABI element output") };
            }
            match cursor.values.next() {
                Some(entry) => {
                    // SAFETY: both aligned, disjoint slots are uninitialized and
                    // callbacks establish independent ownership before success.
                    unsafe { (cursor.key.clone)(entry.key.0.pointer(), out_key) };
                    unsafe { (cursor.value.clone)(entry.value.pointer(), out_value) };
                    unsafe { present.write(1) };
                }
                None => unsafe { present.write(0) },
            }
            ABI_OK
        }

        /// # Safety
        /// cursor is one owned Map cursor returned by map_cursor_take, consumed
        /// exactly once; null is a no-op. Remaining entries are destroyed.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dever_rt_v1_map_cursor_release(cursor: *mut c_void) {
            if !cursor.is_null() {
                // SAFETY: caller transfers one Box<MapCursor> owner.
                drop(unsafe { Box::from_raw(cursor as *mut MapCursor) });
            }
        }
    }
}
