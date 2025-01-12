//! A simple wrapper around the C library's `execvp` function.
//!
//! For examples, see [the repository](https://github.com/faradayio/exec-rs).
//!
//! We'd love to fully integrate this with `std::process::Command`, but
//! that module doesn't export sufficient hooks to allow us to add a new
//! way to execute a program.

extern crate errno;
extern crate libc;

use errno::{errno, Errno};
use std::error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::iter::{IntoIterator, Iterator};
use std::ptr;

/// Represents an error calling `exec`.
///
/// This is marked `#[must_use]`, which is unusual for error types.
/// Normally, the fact that `Result` is marked in this fashion is
/// sufficient, but in this case, this error is returned bare from
/// functions that only return a result if they fail.
#[derive(Debug)]
#[must_use]
pub enum Error {
    /// One of the strings passed to `execv` contained an internal null byte
    /// and can't be passed correctly to C.
    NullByteInArgument,
    /// An error was returned by the system.
    Errno(Errno),
}

impl error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::NullByteInArgument => write!(f, "interior NUL byte in string argument to exec"),
            Error::Errno(err) => write!(f, "couldn't exec process: {}", err),
        }
    }
}

impl From<Errno> for Error {
    fn from(err: Errno) -> Error {
        Error::Errno(err)
    }
}

/// Like `try!`, but it just returns the error directly without wrapping it
/// in `Err`.  For functions that only return if something goes wrong.
macro_rules! exec_try {
    ( $ expr : expr ) => {
        match $expr {
            Ok(val) => val,
            Err(err) => return From::from(err),
        }
    };
}

/// Run `program` with `args`, completely replacing the currently running
/// program.  If it returns at all, it always returns an error.
///
/// Note that `program` and the first element of `args` will normally be
/// identical.  The former is the program we ask the operating system to
/// run, and the latter is the value that will show up in `argv[0]` when
/// the program executes.  On POSIX systems, these can technically be
/// completely different, and we've perserved that much of the low-level
/// API here.
///
/// # Examples
///
/// ```no_run
/// let err = exec::execvp("echo", &["echo", "foo"]);
/// println!("Error: {}", err);
/// ```
pub fn execvp<S, I>(program: S, args: I) -> Error
where
    S: AsRef<OsStr>,
    I: IntoIterator,
    I::Item: AsRef<OsStr>,
{
    execvp_impl(program, args)
}

#[cfg(unix)]
fn execvp_impl<S, I>(program: S, args: I) -> Error
where
    S: AsRef<OsStr>,
    I: IntoIterator,
    I::Item: AsRef<OsStr>,
{
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // Add null terminations to our strings and our argument array,
    // converting them into a C-compatible format.
    let program_cstring =
        exec_try!(CString::new(program.as_ref().as_bytes()).map_err(|_| Error::NullByteInArgument));
    let arg_cstrings = exec_try!(args
        .into_iter()
        .map(|arg| { CString::new(arg.as_ref().as_bytes()).map_err(|_| Error::NullByteInArgument) })
        .collect::<Result<Vec<_>, _>>());
    let mut arg_charptrs: Vec<_> = arg_cstrings.iter().map(|arg| arg.as_ptr()).collect();
    arg_charptrs.push(ptr::null());

    // Use an `unsafe` block so that we can call directly into C.
    let res = unsafe { libc::execvp(program_cstring.as_ptr(), arg_charptrs.as_ptr()) };

    // Handle our error result.
    if res < 0 {
        Error::Errno(errno())
    } else {
        // Should never happen.
        panic!("execvp returned unexpectedly")
    }
}

#[cfg(windows)]
fn execvp_impl<S, I>(program: S, args: I) -> Error
where
    S: AsRef<OsStr>,
    I: IntoIterator,
    I::Item: AsRef<OsStr>,
{
    use std::os::windows::ffi::OsStrExt;

    let wcstring = |s: &OsStr| -> Result<Vec<u16>, Error> {
        let mut vec: Vec<u16> = s.encode_wide().collect();
        if vec.iter().any(|&x| x == 0) {
            // We have an interior null.
            // The Unix impl includes a NulError, but that's only constructible using CString.
            Err(Error::NullByteInArgument)
        } else {
            vec.push(0); // append null terminator
            Ok(vec)
        }
    };

    let program_wide = exec_try!(wcstring(program.as_ref()));
    let args_wide = exec_try!(args
        .into_iter()
        .map(|arg| wcstring(arg.as_ref()))
        .collect::<Result<Vec<_>, _>>());
    let mut arg_ptrs: Vec<_> = args_wide.iter().map(|arg| arg.as_ptr()).collect();
    arg_ptrs.push(ptr::null());

    let res = unsafe { libc::wexecvp(program_wide.as_ptr(), arg_ptrs.as_ptr()) };

    // Handle our error result.
    if res < 0 {
        Error::Errno(errno())
    } else {
        // Should never happen.
        panic!("wexecvp returned unexpectedly")
    }
}

/// Build a command to execute.  This has an API which is deliberately
/// similar to `std::process::Command`.
///
/// ```no_run
/// let err = exec::Command::new("echo")
///     .arg("hello")
///     .arg("world")
///     .exec();
/// println!("Error: {}", err);
/// ```
///
/// If the `exec` function succeeds, it will never return.
pub struct Command {
    /// The program name and arguments, in typical C `argv` style.
    argv: Vec<OsString>,
}

impl Command {
    /// Create a new command builder, specifying the program to run.  The
    /// program will be searched for using the usual rules for `PATH`.
    pub fn new<S: AsRef<OsStr>>(program: S) -> Command {
        Command {
            argv: vec![program.as_ref().to_owned()],
        }
    }

    /// Add an argument to the command builder.  This can be chained.
    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Command {
        self.argv.push(arg.as_ref().to_owned());
        self
    }

    /// Add multiple arguments to the command builder.  This can be
    /// chained.
    ///
    /// ```no_run
    /// let err = exec::Command::new("echo")
    ///     .args(&["hello", "world"])
    ///     .exec();
    /// println!("Error: {}", err);
    /// ```
    pub fn args<S: AsRef<OsStr>>(&mut self, args: &[S]) -> &mut Command {
        for arg in args {
            self.arg(arg.as_ref());
        }
        self
    }

    /// Execute the command we built.  If this function succeeds, it will
    /// never return.
    pub fn exec(&mut self) -> Error {
        execvp(&self.argv[0], &self.argv)
    }
}
