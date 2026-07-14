//! Contained Windows execution for one-shot helper processes.

use cao_application::{
    CancellationProbe, FailureKind, OneShotProcess, OperationId, PortFailure, PortId, ProcessFacts,
    ProcessRequest,
};
use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Windows implementation of the bounded one-shot process capability.
#[derive(Default)]
pub(crate) struct WindowsOneShotProcess;

impl OneShotProcess for WindowsOneShotProcess {
    /// Executes one helper inside a kill-on-close Windows Job Object.
    ///
    /// The child starts suspended so it cannot create descendants before containment is
    /// established. Only its three standard handles are inherited, and both output pipes
    /// are drained to EOF even after their independent retention limits are reached.
    fn execute(
        &mut self,
        request: &ProcessRequest,
        cancellation: &dyn CancellationProbe,
    ) -> Result<ProcessFacts, PortFailure> {
        validate_request(request)?;
        let command_line = build_command_line(request.executable(), request.arguments())?;
        let environment = build_environment(request.environment())?;

        execute_contained(request, command_line, environment, cancellation)
    }
}

/// Routes the safe platform adapter through the private audited Win32 boundary.
pub(super) fn execute_contained(
    request: &ProcessRequest,
    command_line: Vec<u16>,
    environment: Vec<u16>,
    cancellation: &dyn CancellationProbe,
) -> Result<ProcessFacts, PortFailure> {
    crate::process::windows_api::execute(
        request,
        command_line,
        environment,
        cancellation,
        PROCESS_POLL_INTERVAL,
    )
    .map_err(|error| match error {
        ExecutionError::InvalidInput(diagnostic) => {
            process_failure(FailureKind::InvalidInput, request.executable(), diagnostic)
        }
        ExecutionError::Io(error) => process_failure(
            FailureKind::Io,
            request.executable(),
            format!("contained helper execution failed: {error}"),
        ),
        ExecutionError::ReaderPanicked(stream) => process_failure(
            FailureKind::Internal,
            request.executable(),
            format!("{stream} drain thread panicked"),
        ),
    })
}

/// Failures internal to the safe process adapter before stable port mapping.
enum ExecutionError {
    InvalidInput(&'static str),
    Io(io::Error),
    ReaderPanicked(&'static str),
}

impl From<io::Error> for ExecutionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Rejects inputs that would otherwise acquire ambient search or directory meaning.
fn validate_request(request: &ProcessRequest) -> Result<(), PortFailure> {
    if !request.executable().is_absolute() {
        return Err(process_failure(
            FailureKind::InvalidInput,
            request.executable(),
            "helper executable path must be absolute",
        ));
    }
    if !request.working_directory().is_absolute() {
        return Err(process_failure(
            FailureKind::InvalidInput,
            request.working_directory(),
            "helper working directory must be absolute",
        ));
    }
    Ok(())
}

/// Builds a mutable Windows command line with CRT-compatible argument quoting.
fn build_command_line(executable: &Path, arguments: &[String]) -> Result<Vec<u16>, PortFailure> {
    let mut command = Vec::new();
    append_quoted_argument(&mut command, executable.as_os_str())
        .map_err(|diagnostic| process_failure(FailureKind::InvalidInput, executable, diagnostic))?;
    for argument in arguments {
        command.push(' ' as u16);
        append_quoted_argument(&mut command, OsStr::new(argument)).map_err(|diagnostic| {
            process_failure(FailureKind::InvalidInput, executable, diagnostic)
        })?;
    }
    command.push(0);
    Ok(command)
}

/// Appends one argument using the quoting rules consumed by CommandLineToArgvW/CRT parsers.
fn append_quoted_argument(target: &mut Vec<u16>, argument: &OsStr) -> Result<(), &'static str> {
    let units: Vec<u16> = argument.encode_wide().collect();
    if units.contains(&0) {
        return Err("helper argument contains an embedded NUL");
    }
    let needs_quotes = units.is_empty()
        || units
            .iter()
            .any(|unit| *unit == 0x20 || *unit == 0x09 || *unit == 0x22);
    if !needs_quotes {
        target.extend(units);
        return Ok(());
    }

    target.push(b'"' as u16);
    let mut backslashes = 0;
    for unit in units {
        if unit == b'\\' as u16 {
            backslashes += 1;
            continue;
        }
        if unit == b'"' as u16 {
            target.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
        } else {
            target.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
        }
        backslashes = 0;
        target.push(unit);
    }
    // Backslashes immediately before the closing quote must themselves be escaped.
    target.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    target.push(b'"' as u16);
    Ok(())
}

/// Builds the complete sorted Unicode environment block supplied to CreateProcessW.
fn build_environment(environment: &[(String, String)]) -> Result<Vec<u16>, PortFailure> {
    let mut entries: Vec<(Vec<u16>, Vec<u16>)> = environment
        .iter()
        .map(|(key, value)| {
            let key: Vec<u16> = OsStr::new(key).encode_wide().collect();
            let value: Vec<u16> = OsStr::new(value).encode_wide().collect();
            if key.is_empty()
                || key.contains(&(b'=' as u16))
                || key.contains(&0)
                || value.contains(&0)
            {
                return Err(process_failure(
                    FailureKind::InvalidInput,
                    Path::new("<environment>"),
                    "helper environment contains an invalid name or embedded NUL",
                ));
            }
            Ok((key, value))
        })
        .collect::<Result<_, _>>()?;
    entries.sort_by(|left, right| {
        let left = String::from_utf16_lossy(&left.0).to_uppercase();
        let right = String::from_utf16_lossy(&right.0).to_uppercase();
        left.cmp(&right)
    });
    if entries
        .windows(2)
        .any(|pair| pair[0].0.eq_ignore_ascii_case(&pair[1].0))
    {
        return Err(process_failure(
            FailureKind::InvalidInput,
            Path::new("<environment>"),
            "helper environment contains duplicate names",
        ));
    }

    let mut block = Vec::new();
    for (key, value) in entries {
        block.extend(key);
        block.push(b'=' as u16);
        block.extend(value);
        block.push(0);
    }
    // Windows requires an additional NUL after the final entry (or two for an empty block).
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    Ok(block)
}

/// Compares UTF-16 environment names using Windows' case-insensitive ASCII convention.
trait WideAsciiCaseEq {
    fn eq_ignore_ascii_case(&self, other: &Self) -> bool;
}

impl WideAsciiCaseEq for Vec<u16> {
    fn eq_ignore_ascii_case(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self.iter().zip(other).all(|(left, right)| {
                if *left > 0x7f || *right > 0x7f {
                    left == right
                } else {
                    (*left as u8).eq_ignore_ascii_case(&(*right as u8))
                }
            })
    }
}

/// Maps one native process failure to the stable application port contract.
fn process_failure(
    kind: FailureKind,
    subject: &Path,
    diagnostic: impl Into<String>,
) -> PortFailure {
    PortFailure::new(
        PortId::Process,
        OperationId::ExecuteProcess,
        kind,
        diagnostic,
    )
    .with_subject(subject.display().to_string())
}

/// Direct Win32 process and Job Object calls behind an owned safe interface.
///
/// Every raw handle returned by Windows is immediately placed in an owner. The extended
/// startup attribute list points only at storage that outlives CreateProcessW. A suspended
/// child is assigned to the kill-on-close job before ResumeThread, preventing an uncontained
/// descendant race. Pipe read handles become `File`s exactly once and are drained on threads.
mod windows_api {
    use super::ExecutionError;
    use cao_application::{
        CancellationProbe, ProcessFacts, ProcessOutput, ProcessRequest, ProcessTermination,
    };
    use std::ffi::{OsStr, c_void};
    use std::fs::File;
    use std::io::{self, Read};
    use std::mem::{self, size_of};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use std::ptr;
    use std::thread;
    use std::time::{Duration, Instant};

    type Bool = i32;
    type Dword = u32;
    type Handle = *mut c_void;

    const TRUE: Bool = 1;
    const INVALID_HANDLE_VALUE: Handle = -1_isize as Handle;
    const STARTF_USESTDHANDLES: Dword = 0x0000_0100;
    const CREATE_SUSPENDED: Dword = 0x0000_0004;
    const CREATE_UNICODE_ENVIRONMENT: Dword = 0x0000_0400;
    const EXTENDED_STARTUPINFO_PRESENT: Dword = 0x0008_0000;
    const CREATE_NO_WINDOW: Dword = 0x0800_0000;
    const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x0002_0002;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: Dword = 0x0000_2000;
    const JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION: Dword = 1;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: Dword = 9;
    const WAIT_OBJECT_0: Dword = 0;
    const WAIT_TIMEOUT: Dword = 258;
    const INFINITE: Dword = u32::MAX;
    const GENERIC_READ: Dword = 0x8000_0000;
    const FILE_SHARE_READ: Dword = 0x1;
    const FILE_SHARE_WRITE: Dword = 0x2;
    const OPEN_EXISTING: Dword = 3;
    const FILE_ATTRIBUTE_NORMAL: Dword = 0x80;
    const HANDLE_FLAG_INHERIT: Dword = 0x1;

    #[repr(C)]
    struct SecurityAttributes {
        length: Dword,
        security_descriptor: *mut c_void,
        inherit_handle: Bool,
    }

    #[repr(C)]
    struct StartupInfoW {
        cb: Dword,
        reserved: *mut u16,
        desktop: *mut u16,
        title: *mut u16,
        x: Dword,
        y: Dword,
        x_size: Dword,
        y_size: Dword,
        x_count_chars: Dword,
        y_count_chars: Dword,
        fill_attribute: Dword,
        flags: Dword,
        show_window: u16,
        reserved2_bytes: u16,
        reserved2: *mut u8,
        standard_input: Handle,
        standard_output: Handle,
        standard_error: Handle,
    }

    #[repr(C)]
    struct StartupInfoExW {
        startup_info: StartupInfoW,
        attribute_list: *mut c_void,
    }

    #[repr(C)]
    struct ProcessInformation {
        process: Handle,
        thread: Handle,
        process_id: Dword,
        thread_id: Dword,
    }

    #[repr(C)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: Dword,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: Dword,
        affinity: usize,
        priority_class: Dword,
        scheduling_class: Dword,
    }

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct JobObjectExtendedLimitInformation {
        basic_limit_information: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[repr(C)]
    struct JobObjectBasicAccountingInformation {
        total_user_time: i64,
        total_kernel_time: i64,
        this_period_total_user_time: i64,
        this_period_total_kernel_time: i64,
        total_page_fault_count: Dword,
        total_processes: Dword,
        active_processes: Dword,
        total_terminated_processes: Dword,
    }

    pub(super) struct OwnedHandle(Handle);

    impl OwnedHandle {
        /// Takes ownership of one non-sentinel handle returned by a private Win32 wrapper.
        fn from_raw(handle: Handle) -> io::Result<Self> {
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                Err(io::Error::last_os_error())
            } else {
                Ok(Self(handle))
            }
        }

        /// Returns the borrowed raw handle for the duration of one Win32 call.
        fn raw(&self) -> Handle {
            self.0
        }

        /// Transfers ownership of a pipe handle into `File` exactly once.
        fn into_file(self) -> File {
            let raw = self.0;
            mem::forget(self);
            // SAFETY: `raw` is a valid uniquely owned pipe handle and ownership moves to File.
            unsafe { File::from_raw_handle(raw as RawHandle) }
        }
    }

    impl Drop for OwnedHandle {
        /// Closes the uniquely owned Win32 handle exactly once.
        fn drop(&mut self) {
            // SAFETY: this owner contains one valid closeable handle and closes it once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    struct AttributeList {
        storage: Vec<usize>,
        pointer: *mut c_void,
    }

    impl AttributeList {
        /// Creates a one-entry process attribute list containing only inherited handles.
        fn for_handles(handles: &[Handle]) -> io::Result<Self> {
            let mut bytes = 0;
            // SAFETY: the documented sizing call writes only the required byte count.
            unsafe {
                InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut bytes);
            }
            let words = bytes.div_ceil(size_of::<usize>());
            let mut storage = vec![0_usize; words];
            let pointer = storage.as_mut_ptr().cast();
            // SAFETY: storage is aligned, writable, and at least the size returned above.
            if unsafe { InitializeProcThreadAttributeList(pointer, 1, 0, &mut bytes) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: both the list and handle array remain alive through CreateProcessW.
            if unsafe {
                UpdateProcThreadAttribute(
                    pointer,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                    handles.as_ptr().cast_mut().cast(),
                    size_of_val(handles),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            } == 0
            {
                // SAFETY: initialization succeeded and therefore requires deletion.
                unsafe { DeleteProcThreadAttributeList(pointer) };
                return Err(io::Error::last_os_error());
            }
            Ok(Self { storage, pointer })
        }
    }

    impl Drop for AttributeList {
        /// Releases native attribute-list state before its backing allocation is freed.
        fn drop(&mut self) {
            // SAFETY: pointer denotes one successfully initialized attribute list.
            unsafe { DeleteProcThreadAttributeList(self.pointer) };
            // Keep the backing allocation observably live until after deletion.
            std::hint::black_box(&self.storage);
        }
    }

    struct Pipe {
        read: OwnedHandle,
        write: OwnedHandle,
    }

    /// Executes one already-validated process request under full native containment.
    pub(super) fn execute(
        request: &ProcessRequest,
        mut command_line: Vec<u16>,
        mut environment: Vec<u16>,
        cancellation: &dyn CancellationProbe,
        poll_interval: Duration,
    ) -> Result<ProcessFacts, ExecutionError> {
        let stdout = create_pipe()?;
        let stderr = create_pipe()?;
        let standard_input = open_inheritable_null()?;
        let inherited = [standard_input.raw(), stdout.write.raw(), stderr.write.raw()];
        let attributes = AttributeList::for_handles(&inherited)?;
        let job = create_kill_on_close_job()?;

        let executable = null_terminated(request.executable().as_os_str())?;
        let working_directory = null_terminated(request.working_directory().as_os_str())?;
        // SAFETY: all structures are zero-initialized POD before documented fields are set.
        let mut startup: StartupInfoExW = unsafe { mem::zeroed() };
        startup.startup_info.cb = size_of::<StartupInfoExW>() as Dword;
        startup.startup_info.flags = STARTF_USESTDHANDLES;
        startup.startup_info.standard_input = standard_input.raw();
        startup.startup_info.standard_output = stdout.write.raw();
        startup.startup_info.standard_error = stderr.write.raw();
        startup.attribute_list = attributes.pointer;
        // SAFETY: ProcessInformation is output-only POD initialized by CreateProcessW.
        let mut information: ProcessInformation = unsafe { mem::zeroed() };
        let creation_flags = CREATE_SUSPENDED
            | CREATE_UNICODE_ENVIRONMENT
            | EXTENDED_STARTUPINFO_PRESENT
            | CREATE_NO_WINDOW;
        // SAFETY: every pointer references initialized storage alive for this call; the command
        // line is mutable as required, and inherited handles are restricted by the attribute list.
        if unsafe {
            CreateProcessW(
                executable.as_ptr(),
                command_line.as_mut_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                TRUE,
                creation_flags,
                environment.as_mut_ptr().cast(),
                working_directory.as_ptr(),
                &startup.startup_info,
                &mut information,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let process = OwnedHandle::from_raw(information.process)?;
        let primary_thread = OwnedHandle::from_raw(information.thread)?;

        if let Err(assignment_error) = assign_process_to_job(&job, &process) {
            // The job does not own an assignment failure, so explicitly terminate the still-
            // suspended child before allowing its last process handle to leave this scope.
            terminate_process(&process)?;
            return Err(assignment_error.into());
        }

        // Parent copies of inherited stream ends must close before drains can observe EOF.
        drop(standard_input);
        drop(stdout.write);
        drop(stderr.write);
        let stdout_reader = spawn_drain(stdout.read.into_file(), request.output_limit(), "stdout")?;
        let stderr_reader = spawn_drain(stderr.read.into_file(), request.output_limit(), "stderr")?;

        // SAFETY: the primary thread remains suspended and its handle is valid.
        if unsafe { ResumeThread(primary_thread.raw()) } == u32::MAX {
            let resume_error = io::Error::last_os_error();
            terminate_job(&job)?;
            // The contained child has closed both pipe writers, so complete the mandatory drains.
            let _ = join_drain(stdout_reader, "stdout")?;
            let _ = join_drain(stderr_reader, "stderr")?;
            return Err(resume_error.into());
        }

        let started = Instant::now();
        let mut primary_exited = false;
        let termination = loop {
            if cancellation.is_cancelled() {
                terminate_job(&job)?;
                break ProcessTermination::Cancelled;
            }
            if started.elapsed() >= request.timeout() {
                terminate_job(&job)?;
                break ProcessTermination::TimedOut;
            }
            let remaining = request.timeout().saturating_sub(started.elapsed());
            let wait = poll_interval.min(remaining);
            if primary_exited {
                if active_process_count(&job)? == 0 {
                    let mut exit_code = 0;
                    // SAFETY: process remains a valid query handle after termination.
                    if unsafe { GetExitCodeProcess(process.raw(), &mut exit_code) } == 0 {
                        return Err(io::Error::last_os_error().into());
                    }
                    break ProcessTermination::Exited(Some(exit_code as i32));
                }
                thread::sleep(wait);
                continue;
            }

            // SAFETY: the primary process handle is valid and waitable; descendants are checked
            // separately through the job's active-process accounting after the helper exits.
            match unsafe { WaitForSingleObject(process.raw(), duration_millis(wait)) } {
                WAIT_OBJECT_0 => primary_exited = true,
                WAIT_TIMEOUT => {}
                _ => return Err(io::Error::last_os_error().into()),
            }
        };

        let stdout = join_drain(stdout_reader, "stdout")?;
        let stderr = join_drain(stderr_reader, "stderr")?;
        Ok(ProcessFacts::new(termination, stdout, stderr))
    }

    /// Assigns the still-suspended primary process to its kill-on-close job.
    pub(super) fn assign_process_to_job(
        job: &OwnedHandle,
        process: &OwnedHandle,
    ) -> io::Result<()> {
        // SAFETY: Both handles are live for this call and identify a Job Object and process.
        if unsafe { self::AssignProcessToJobObject(job.raw(), process.raw()) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Creates an anonymous inheritable pipe while retaining a private parent read end.
    fn create_pipe() -> io::Result<Pipe> {
        let mut attributes = SecurityAttributes {
            length: size_of::<SecurityAttributes>() as Dword,
            security_descriptor: ptr::null_mut(),
            inherit_handle: TRUE,
        };
        let mut read = ptr::null_mut();
        let mut write = ptr::null_mut();
        // SAFETY: output pointers and security attributes are valid for the duration of the call.
        if unsafe { CreatePipe(&mut read, &mut write, &mut attributes, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let read = OwnedHandle::from_raw(read)?;
        let write = OwnedHandle::from_raw(write)?;
        // SAFETY: the read handle is valid; clearing inheritance cannot broaden access.
        if unsafe { SetHandleInformation(read.raw(), HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Pipe { read, write })
    }

    /// Opens an inheritable read-only NUL handle for the helper's standard input.
    fn open_inheritable_null() -> io::Result<OwnedHandle> {
        let mut attributes = SecurityAttributes {
            length: size_of::<SecurityAttributes>() as Dword,
            security_descriptor: ptr::null_mut(),
            inherit_handle: TRUE,
        };
        let name: Vec<u16> = OsStr::new("NUL").encode_wide().chain(Some(0)).collect();
        // SAFETY: the path and security attributes are valid and NUL is a kernel device.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &mut attributes,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        OwnedHandle::from_raw(handle)
    }

    /// Creates a Job Object whose close operation terminates every assigned descendant.
    fn create_kill_on_close_job() -> io::Result<OwnedHandle> {
        // SAFETY: null security/name requests a fresh unnamed Job Object.
        let handle = unsafe { CreateJobObjectW(ptr::null_mut(), ptr::null()) };
        let job = OwnedHandle::from_raw(handle)?;
        // SAFETY: zero is a valid baseline for the documented limit-information structure.
        let mut limits: JobObjectExtendedLimitInformation = unsafe { mem::zeroed() };
        limits.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: job and information pointers are valid with the exact documented byte size.
        if unsafe {
            SetInformationJobObject(
                job.raw(),
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                (&limits as *const JobObjectExtendedLimitInformation).cast(),
                size_of::<JobObjectExtendedLimitInformation>() as Dword,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Returns how many primary or descendant processes remain active in the job.
    fn active_process_count(job: &OwnedHandle) -> io::Result<Dword> {
        // SAFETY: zero is a valid output baseline for the documented accounting structure.
        let mut accounting: JobObjectBasicAccountingInformation = unsafe { mem::zeroed() };
        // SAFETY: job and output pointers are valid with the exact documented byte size.
        if unsafe {
            QueryInformationJobObject(
                job.raw(),
                JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION,
                (&mut accounting as *mut JobObjectBasicAccountingInformation).cast(),
                size_of::<JobObjectBasicAccountingInformation>() as Dword,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(accounting.active_processes)
    }

    /// Terminates every process in a contained job and waits until all handles close.
    fn terminate_job(job: &OwnedHandle) -> io::Result<()> {
        // SAFETY: job is valid and termination applies only to its assigned process tree.
        if unsafe { TerminateJobObject(job.raw(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a Job Object is waitable and becomes signaled when all processes exit.
        if unsafe { WaitForSingleObject(job.raw(), INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Terminates an unassigned suspended child after containment setup fails.
    fn terminate_process(process: &OwnedHandle) -> io::Result<()> {
        // SAFETY: process is a valid process handle created suspended by this adapter.
        if unsafe { TerminateProcess(process.raw(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the process handle is waitable and remains owned throughout the wait.
        if unsafe { WaitForSingleObject(process.raw(), INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Drains one pipe to EOF while retaining at most `limit` leading bytes.
    fn spawn_drain(
        mut stream: File,
        limit: usize,
        name: &'static str,
    ) -> io::Result<thread::JoinHandle<io::Result<ProcessOutput>>> {
        thread::Builder::new()
            .name(format!("tracetide-process-{name}"))
            .spawn(move || {
                let mut retained = Vec::with_capacity(limit.min(8 * 1024));
                let mut truncated = false;
                let mut buffer = [0_u8; 8 * 1024];
                loop {
                    let count = stream.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    let available = limit.saturating_sub(retained.len());
                    let keep = available.min(count);
                    retained.extend_from_slice(&buffer[..keep]);
                    truncated |= keep < count;
                }
                Ok(ProcessOutput::new(retained, truncated))
            })
    }

    /// Joins one mandatory stream drain without allowing a helper to outlive output capture.
    fn join_drain(
        reader: thread::JoinHandle<io::Result<ProcessOutput>>,
        stream: &'static str,
    ) -> Result<ProcessOutput, ExecutionError> {
        reader
            .join()
            .map_err(|_| ExecutionError::ReaderPanicked(stream))?
            .map_err(ExecutionError::Io)
    }

    /// Encodes a path-like Windows string with the terminal NUL required by Win32.
    fn null_terminated(value: &OsStr) -> Result<Vec<u16>, ExecutionError> {
        let mut units: Vec<u16> = value.encode_wide().collect();
        if units.contains(&0) {
            return Err(ExecutionError::InvalidInput(
                "helper path contains an embedded NUL",
            ));
        }
        units.push(0);
        Ok(units)
    }

    /// Converts a bounded polling duration to the DWORD millisecond wait contract.
    fn duration_millis(duration: Duration) -> Dword {
        duration.as_millis().clamp(1, u128::from(u32::MAX - 1)) as Dword
    }

    #[link(name = "kernel32")]
    // SAFETY: These private declarations match the documented Win32 ABI and are called only
    // by the checked wrappers above with live buffers, initialized records, and owned handles.
    unsafe extern "system" {
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
        fn CloseHandle(object: Handle) -> Bool;
        fn CreateFileW(
            file_name: *const u16,
            desired_access: Dword,
            share_mode: Dword,
            security_attributes: *mut SecurityAttributes,
            creation_disposition: Dword,
            flags_and_attributes: Dword,
            template_file: Handle,
        ) -> Handle;
        fn CreateJobObjectW(attributes: *mut SecurityAttributes, name: *const u16) -> Handle;
        fn CreatePipe(
            read_pipe: *mut Handle,
            write_pipe: *mut Handle,
            pipe_attributes: *mut SecurityAttributes,
            size: Dword,
        ) -> Bool;
        fn CreateProcessW(
            application_name: *const u16,
            command_line: *mut u16,
            process_attributes: *mut SecurityAttributes,
            thread_attributes: *mut SecurityAttributes,
            inherit_handles: Bool,
            creation_flags: Dword,
            environment: *mut c_void,
            current_directory: *const u16,
            startup_info: *const StartupInfoW,
            process_information: *mut ProcessInformation,
        ) -> Bool;
        fn DeleteProcThreadAttributeList(attribute_list: *mut c_void);
        fn GetExitCodeProcess(process: Handle, exit_code: *mut Dword) -> Bool;
        fn InitializeProcThreadAttributeList(
            attribute_list: *mut c_void,
            attribute_count: Dword,
            flags: Dword,
            size: *mut usize,
        ) -> Bool;
        fn QueryInformationJobObject(
            job: Handle,
            information_class: Dword,
            information: *mut c_void,
            information_length: Dword,
            return_length: *mut Dword,
        ) -> Bool;
        fn ResumeThread(thread: Handle) -> Dword;
        fn SetHandleInformation(object: Handle, mask: Dword, flags: Dword) -> Bool;
        fn SetInformationJobObject(
            job: Handle,
            information_class: Dword,
            information: *const c_void,
            information_length: Dword,
        ) -> Bool;
        fn TerminateJobObject(job: Handle, exit_code: Dword) -> Bool;
        fn TerminateProcess(process: Handle, exit_code: Dword) -> Bool;
        fn UpdateProcThreadAttribute(
            attribute_list: *mut c_void,
            flags: Dword,
            attribute: usize,
            value: *mut c_void,
            size: usize,
            previous_value: *mut c_void,
            return_size: *mut usize,
        ) -> Bool;
        fn WaitForSingleObject(object: Handle, milliseconds: Dword) -> Dword;
    }
}

#[cfg(test)]
mod tests {
    use super::WindowsOneShotProcess;
    use cao_application::{
        CancellationProbe, FailureKind, OneShotProcess, ProcessRequest, ProcessTermination,
    };
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Duration;

    const PROBE_MODE: &str = "CAO_PROCESS_TEST_MODE";
    const SENTINEL_PATH: &str = "CAO_PROCESS_TEST_SENTINEL";

    struct NeverCancelled;

    impl CancellationProbe for NeverCancelled {
        fn is_cancelled(&self) -> bool {
            false
        }
    }

    struct AlwaysCancelled;

    impl CancellationProbe for AlwaysCancelled {
        fn is_cancelled(&self) -> bool {
            true
        }
    }

    #[test]
    fn relative_executable_path_is_rejected_without_launching() {
        let request = ProcessRequest::new(
            PathBuf::from("helper.exe"),
            test_working_directory(),
            Vec::new(),
            Vec::new(),
            Duration::from_secs(1),
            1024,
        );

        let failure = WindowsOneShotProcess
            .execute(&request, &NeverCancelled)
            .expect_err("relative executables must not inherit search-path meaning");

        assert_eq!(failure.kind(), FailureKind::InvalidInput);
    }

    #[test]
    fn child_receives_only_the_explicit_environment() {
        let request = probe_request("environment", Duration::from_secs(5), 4096, Vec::new());

        let facts = WindowsOneShotProcess
            .execute(&request, &NeverCancelled)
            .expect("environment probe should run");

        assert_eq!(facts.termination(), ProcessTermination::Exited(Some(0)));
    }

    #[test]
    fn stdout_and_stderr_are_drained_with_independent_retention_bounds() {
        let request = probe_request("output", Duration::from_secs(5), 32, Vec::new());

        let facts = WindowsOneShotProcess
            .execute(&request, &NeverCancelled)
            .expect("output probe should run");

        assert_eq!(facts.termination(), ProcessTermination::Exited(Some(0)));
        assert_eq!(facts.stdout().bytes().len(), 32);
        assert_eq!(facts.stderr().bytes().len(), 32);
        assert!(facts.stdout().was_truncated());
        assert!(facts.stderr().was_truncated());
    }

    #[test]
    fn nonzero_helper_exit_is_returned_as_structured_process_facts() {
        let request = probe_request("exit-23", Duration::from_secs(5), 4096, Vec::new());

        let facts = WindowsOneShotProcess
            .execute(&request, &NeverCancelled)
            .expect("nonzero exit is a process fact rather than an adapter failure");

        assert_eq!(facts.termination(), ProcessTermination::Exited(Some(23)));
    }

    #[test]
    fn elapsed_timeout_terminates_the_contained_helper() {
        let request = probe_request("sleep", Duration::from_millis(50), 4096, Vec::new());

        let facts = WindowsOneShotProcess
            .execute(&request, &NeverCancelled)
            .expect("timeout should be successful control flow");

        assert_eq!(facts.termination(), ProcessTermination::TimedOut);
    }

    #[test]
    fn accepted_cancellation_terminates_the_contained_helper() {
        let request = probe_request("sleep", Duration::from_secs(5), 4096, Vec::new());

        let facts = WindowsOneShotProcess
            .execute(&request, &AlwaysCancelled)
            .expect("cancellation should be successful control flow");

        assert_eq!(facts.termination(), ProcessTermination::Cancelled);
    }

    #[test]
    fn timeout_terminates_descendants_before_they_can_escape_the_job() {
        let sentinel = std::env::temp_dir().join(format!(
            "cao-process-descendant-{}-{}.sentinel",
            std::process::id(),
            unique_test_id()
        ));
        let ready = sentinel.with_extension("ready");
        let request = probe_request(
            "descendant-parent",
            Duration::from_millis(100),
            4096,
            vec![(
                SENTINEL_PATH.to_owned(),
                sentinel.to_string_lossy().into_owned(),
            )],
        );

        let facts = WindowsOneShotProcess
            .execute(&request, &NeverCancelled)
            .expect("descendant probe should time out through the job");
        std::thread::sleep(Duration::from_millis(800));

        assert_eq!(facts.termination(), ProcessTermination::TimedOut);
        assert!(
            ready.exists(),
            "the containment fixture did not spawn its descendant before timeout"
        );
        assert!(
            !sentinel.exists(),
            "a descendant survived Job Object termination and wrote its sentinel"
        );
        // The readiness marker is test coordination residue, not a product artifact.
        let _ = std::fs::remove_file(ready);
    }

    #[test]
    #[allow(
        clippy::zombie_processes,
        reason = "the descendant probe must outlive its primary process so Job Object cleanup is observable"
    )]
    fn process_child_probe() {
        let Some(mode) = std::env::var_os(PROBE_MODE) else {
            // Ordinary parent test discovery must leave the probe inert.
            return;
        };
        match mode.to_string_lossy().as_ref() {
            "environment" => {
                assert_eq!(std::env::var(PROBE_MODE).as_deref(), Ok("environment"));
                assert!(std::env::var_os("PATH").is_none());
            }
            "output" => {
                std::io::stdout()
                    .write_all(&vec![b'A'; 256 * 1024])
                    .expect("stdout probe should write");
                std::io::stderr()
                    .write_all(&vec![b'B'; 256 * 1024])
                    .expect("stderr probe should write");
            }
            "exit-23" => std::process::exit(23),
            "sleep" => std::thread::sleep(Duration::from_secs(30)),
            "descendant-parent" => {
                let sentinel = std::env::var_os(SENTINEL_PATH)
                    .expect("descendant probe should receive a sentinel path");
                // Deliberately do not wait: the parent must remain killable while its
                // descendant proves that Job Object termination covers the whole tree.
                Command::new(std::env::current_exe().expect("test executable should resolve"))
                    .arg("--exact")
                    .arg("process::tests::process_child_probe")
                    .arg("--nocapture")
                    .env(PROBE_MODE, "descendant-child")
                    .env(SENTINEL_PATH, sentinel)
                    .spawn()
                    .expect("contained helper should create its descendant");
                let ready = PathBuf::from(
                    std::env::var_os(SENTINEL_PATH)
                        .expect("descendant probe should retain its sentinel path"),
                )
                .with_extension("ready");
                std::fs::write(ready, b"descendant spawned\n")
                    .expect("descendant fixture should publish readiness");
                std::thread::sleep(Duration::from_secs(30));
            }
            "descendant-child" => {
                let sentinel = std::env::var_os(SENTINEL_PATH)
                    .expect("descendant should receive a sentinel path");
                std::thread::sleep(Duration::from_millis(500));
                std::fs::write(sentinel, b"escaped job\n")
                    .expect("an uncontained descendant should expose itself");
            }
            unexpected => panic!("unexpected process probe mode: {unexpected}"),
        }
    }

    /// Creates a request that reinvokes this test binary as one isolated child probe.
    fn probe_request(
        mode: &str,
        timeout: Duration,
        output_limit: usize,
        mut extra_environment: Vec<(String, String)>,
    ) -> ProcessRequest {
        extra_environment.push((PROBE_MODE.to_owned(), mode.to_owned()));
        ProcessRequest::new(
            std::env::current_exe().expect("test executable should resolve"),
            test_working_directory(),
            vec![
                "--exact".to_owned(),
                "process::tests::process_child_probe".to_owned(),
                "--nocapture".to_owned(),
            ],
            extra_environment,
            timeout,
            output_limit,
        )
    }

    /// Returns a canonical absolute directory acceptable to the production seam.
    fn test_working_directory() -> PathBuf {
        std::env::current_dir()
            .expect("test working directory should resolve")
            .canonicalize()
            .expect("test working directory should canonicalize")
    }

    /// Allocates collision-resistant suffixes for concurrently executing process tests.
    fn unique_test_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }
}
