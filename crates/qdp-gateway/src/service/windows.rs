//! Windows services, of the Service Control Manager. A program's service runs
//! a copy of the program installed under Program Files, which accounts with
//! few rights can read, with arguments that make it run as the service, as
//! the account given, from boot, restarting it when it fails.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::Start;

/// The folder of Program Files that QDP's programs are installed in.
const INSTALL_FOLDER: &str = "QDP";

/// How long to wait after a failure before restarting the service.
const RESTART_DELAY: Duration = Duration::from_secs(5);

/// How long after the last failure the count of failures starts again.
const FAILURE_RESET: Duration = Duration::from_secs(24 * 60 * 60);

/// How long to wait for a service to stop.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// How often to look whether a service has stopped.
const STOP_POLL: Duration = Duration::from_millis(250);

/// How long the service takes to stop once asked, at most, which the system
/// waits for.
const STOP_HINT: Duration = Duration::from_secs(10);

/// The exit code of a service whose program failed.
const FAILED_EXIT_CODE: u32 = 1;

/// The SID of the LocalSystem account.
const SYSTEM_SID: &str = "*S-1-5-18";

/// The SID of the Administrators group.
const ADMINISTRATORS_SID: &str = "*S-1-5-32-544";

/// The SID of the LocalService account.
const LOCAL_SERVICE_SID: &str = "*S-1-5-19";

/// The account a service runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Account {
    /// LocalService: few rights on the machine, and anonymous on the network,
    /// which suits a program that authenticates on its own, as the gateway
    /// does to the directory.
    LocalService,
}

impl Account {
    /// Returns the account's name for the Service Control Manager.
    fn name(self) -> OsString {
        match self {
            Self::LocalService => OsString::from("NT AUTHORITY\\LocalService"),
        }
    }

    /// Returns the account's SID for `icacls`.
    fn sid(self) -> &'static str {
        match self {
            Self::LocalService => LOCAL_SERVICE_SID,
        }
    }
}

/// A program's Windows service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsService {
    /// The service's name, such as `qdp-gateway`.
    pub name: &'static str,
    /// Its name in the Services console, such as `QDP Gateway`.
    pub display_name: &'static str,
    /// What it does, for the Services console.
    pub description: &'static str,
    /// The program's file name, such as `qdp-gateway.exe`.
    pub program: &'static str,
    /// The account it runs as.
    pub account: Account,
}

impl WindowsService {
    /// Installs the service, or updates it if installed, to run the program
    /// with `arguments` from boot, and starts it as `start` says.
    pub fn install(&self, arguments: Vec<OsString>, start: Start) -> Result<(), String> {
        let manager =
            manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
        let access = ServiceAccess::QUERY_STATUS
            | ServiceAccess::START
            | ServiceAccess::STOP
            | ServiceAccess::CHANGE_CONFIG;
        let existing = manager.open_service(self.name, access).ok();
        // A running service holds its program, which cannot be replaced
        // until it stops.
        if let Some(service) = &existing {
            stop(service)?;
        }
        let program = self.service_program()?;
        let info = ServiceInfo {
            name: OsString::from(self.name),
            display_name: OsString::from(self.display_name),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: program,
            launch_arguments: arguments,
            dependencies: Vec::new(),
            account_name: Some(self.account.name()),
            account_password: None,
        };
        let service = match existing {
            Some(service) => {
                service.change_config(&info).map_err(failure)?;
                service
            }
            None => manager.create_service(&info, access).map_err(failure)?,
        };
        service.set_description(self.description).map_err(failure)?;
        let restart = ServiceAction {
            action_type: ServiceActionType::Restart,
            delay: RESTART_DELAY,
        };
        service
            .update_failure_actions(ServiceFailureActions {
                reset_period: ServiceFailureResetPeriod::After(FAILURE_RESET),
                reboot_msg: None,
                command: None,
                actions: Some(vec![restart; 3]),
            })
            .map_err(failure)?;
        if start == Start::Now {
            service.start::<&OsStr>(&[]).map_err(failure)?;
        }
        Ok(())
    }

    /// Stops the service and removes it. The program's copy and its data stay.
    pub fn uninstall(&self) -> Result<(), String> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let service = manager
            .open_service(
                self.name,
                ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
            )
            .map_err(|_| format!("the {} service is not installed", self.name))?;
        stop(&service)?;
        service.delete().map_err(failure)
    }

    /// Returns the service's state, in words.
    pub fn status(&self) -> Result<String, String> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let service = manager
            .open_service(self.name, ServiceAccess::QUERY_STATUS)
            .map_err(|_| format!("the {} service is not installed", self.name))?;
        let status = service.query_status().map_err(failure)?;
        Ok(match status.current_state {
            ServiceState::Running => "running",
            ServiceState::Stopped => "stopped",
            ServiceState::StartPending => "starting",
            ServiceState::StopPending => "stopping",
            ServiceState::Paused | ServiceState::PausePending => "paused",
            ServiceState::ContinuePending => "resuming",
        }
        .to_owned())
    }

    /// Returns the program the service runs: a copy of this one under
    /// Program Files, made unless it runs from there already.
    fn service_program(&self) -> Result<PathBuf, String> {
        let current = std::env::current_exe()
            .map_err(|error| format!("cannot find this program: {error}"))?;
        let folder = program_files()?.join(INSTALL_FOLDER);
        let installed = folder.join(self.program);
        if current.parent() == Some(folder.as_path()) {
            return Ok(current);
        }
        fs::create_dir_all(&folder).map_err(|error| io_failure(&folder, &error))?;
        fs::copy(&current, &installed).map_err(|error| io_failure(&installed, &error))?;
        println!(
            "Installed this program as {}, which the service runs.",
            installed.display()
        );
        Ok(installed)
    }
}

/// Opens the Service Control Manager with `access`.
fn manager(access: ServiceManagerAccess) -> Result<ServiceManager, String> {
    ServiceManager::local_computer(None::<&str>, access).map_err(failure)
}

/// Stops `service` if it runs, waiting until it has.
fn stop(service: &windows_service::service::Service) -> Result<(), String> {
    let status = service.query_status().map_err(failure)?;
    if status.current_state == ServiceState::Stopped {
        return Ok(());
    }
    // A service already stopping cannot be told to stop again.
    let _ = service.stop();
    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if service.query_status().map_err(failure)?.current_state == ServiceState::Stopped {
            return Ok(());
        }
        std::thread::sleep(STOP_POLL);
    }
    Err("the service did not stop in time".to_owned())
}

/// Returns the Program Files folder.
fn program_files() -> Result<PathBuf, String> {
    std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .ok_or_else(|| "cannot find the Program Files folder".to_owned())
}

/// Returns the ProgramData folder, where services keep their data.
pub fn program_data() -> Result<PathBuf, String> {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .ok_or_else(|| "cannot find the ProgramData folder".to_owned())
}

/// Lets only the system, administrators and `account` into `folder`, and what
/// it holds, `account` to change it: no other user may read the secrets a
/// service keeps there.
pub fn restrict_folder(folder: &Path, account: Account) -> Result<(), String> {
    fs::create_dir_all(folder).map_err(|error| io_failure(folder, &error))?;
    let full = |sid: &str| format!("{sid}:(OI)(CI)F");
    run(
        "icacls",
        &[
            folder.as_os_str(),
            OsStr::new("/inheritance:r"),
            OsStr::new("/grant:r"),
            OsStr::new(&full(SYSTEM_SID)),
            OsStr::new(&full(ADMINISTRATORS_SID)),
            OsStr::new(&format!("{}:(OI)(CI)M", account.sid())),
        ],
    )
}

/// Lets `program` receive UDP on `port` through Windows Firewall, under the
/// rule `rule`, replacing any rule of that name.
pub fn allow_inbound_udp(rule: &str, program: &Path, port: u16) -> Result<(), String> {
    remove_firewall_rule(rule);
    run(
        "netsh",
        &[
            OsStr::new("advfirewall"),
            OsStr::new("firewall"),
            OsStr::new("add"),
            OsStr::new("rule"),
            OsStr::new(&format!("name={rule}")),
            OsStr::new("dir=in"),
            OsStr::new("action=allow"),
            OsStr::new("protocol=UDP"),
            OsStr::new(&format!("localport={port}")),
            OsStr::new(&format!("program={}", program.display())),
            OsStr::new("enable=yes"),
        ],
    )
}

/// Removes the Windows Firewall rule `rule`, if there is one.
pub fn remove_firewall_rule(rule: &str) {
    let _ = run(
        "netsh",
        &[
            OsStr::new("advfirewall"),
            OsStr::new("firewall"),
            OsStr::new("delete"),
            OsStr::new("rule"),
            OsStr::new(&format!("name={rule}")),
        ],
    );
}

/// Returns the path of the program a service installed by
/// [`WindowsService::install`] runs.
pub fn installed_program(service: &WindowsService) -> Result<PathBuf, String> {
    Ok(program_files()?.join(INSTALL_FOLDER).join(service.program))
}

/// What a program runs as its service: given a receiver that gets a message
/// when the system asks the service to stop, it runs until then.
pub type ServiceMain = fn(Receiver<()>) -> Result<(), String>;

/// The service the dispatcher runs, which the system's callback finds here.
static SERVICE: OnceLock<(&'static str, ServiceMain)> = OnceLock::new();

/// Runs this program as the service `name`, calling `main` until the system
/// asks the service to stop. Fails unless the Service Control Manager
/// started the program.
pub fn run_as_service(name: &'static str, main: ServiceMain) -> Result<(), String> {
    SERVICE
        .set((name, main))
        .map_err(|_| "the service runs already".to_owned())?;
    service_dispatcher::start(name, dispatcher::entry()).map_err(|error| {
        format!("cannot run as a service; start it with `sc start {name}`: {error}")
    })
}

/// The system's callback that starts the service.
#[allow(unsafe_code)]
mod dispatcher {
    use super::service_main;

    windows_service::define_windows_service!(ffi_service_main, service_main);

    /// Returns the callback the system starts the service with.
    pub(super) fn entry() -> extern "system" fn(u32, *mut *mut u16) {
        ffi_service_main
    }
}

/// Runs the service the dispatcher was given, reporting its state to the
/// system.
fn service_main(_arguments: Vec<OsString>) {
    let Some(&(name, main)) = SERVICE.get() else {
        return;
    };
    let (stop, stopped) = mpsc::channel();
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let Ok(status) = service_control_handler::register(name, handler) else {
        return;
    };
    let report = |state, accepted, exit_code| {
        let _ = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accepted,
            exit_code,
            checkpoint: 0,
            wait_hint: STOP_HINT,
            process_id: None,
        });
    };
    report(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ServiceExitCode::Win32(0),
    );
    let exit_code = match main(stopped) {
        Ok(()) => ServiceExitCode::Win32(0),
        Err(error) => {
            eprintln!("{name}: {error}");
            ServiceExitCode::ServiceSpecific(FAILED_EXIT_CODE)
        }
    };
    report(
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        exit_code,
    );
}

/// Runs `program` with `arguments`, failing with what it said.
fn run(program: &str, arguments: &[&OsStr]) -> Result<(), String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stdout).trim()
        ))
    }
}

/// Returns the message of a Service Control Manager error.
fn failure(error: windows_service::Error) -> String {
    let access_denied = matches!(
        &error,
        windows_service::Error::Winapi(inner) if inner.raw_os_error() == Some(5)
    );
    if access_denied {
        "this needs an administrator: run it from an administrator's terminal".to_owned()
    } else {
        format!("the Service Control Manager failed: {error}")
    }
}

/// Returns the message of a failure to write `path`.
fn io_failure(path: &Path, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        "this needs an administrator: run it from an administrator's terminal".to_owned()
    } else {
        format!("cannot write {}: {error}", path.display())
    }
}
