//! Provides functions which perform shell-like expansions in strings.
//!
//! In particular, the following expansions are supported:
//! * tilde expansion, when `~` in the beginning of a string, like in `"~/some/path"`,
//!   is expanded into the home directory of the current user;
//! * environment expansion, when `$A` or `${B}`, like in `"~/$A/${B}something"`,
//!   are expanded into their values in some environment.
//!
//! The source of external information for these expansions (home directory and environment
//! variables) is called their *context*. The context is provided to these functions as a closure
//! of the respective type.
//!
//! This crate provides both customizable functions, which require their context to be provided
//! explicitly, and wrapper functions which use `std::env::home_dir()` and `std::env::var()`
//! for obtaining home directory and environment variables, respectively.
//!
//! Also there is a "full" expansions function which performs both tilde and environment
//! expansion, but does it correctly: for example, if the string starts with a variable
//! whose value starts with a `~`, then this tilde won't be expanded.
//!
//! All functions return `Cow<str>` because it is possible for their input not to contain anything
//! which triggers the expansion. In that case performing allocations can be avoided.
//!
//! Please note that by default unknown variables in environment expansion are left as they are
//! and are not, for example, substituted with an empty string:
//!
//! ```
//! fn context(_: &str) -> Option<String> { None }
//!
//! assert_eq!(
//!     shellexpand::env_with_context_no_errors("$A $B", context),
//!     "$A $B"
//! );
//! ```
//!
//! Environment expansion context allows for a very fine tweaking of how results should be handled,
//! so it is up to the user to pass a context function which does the necessary thing. For example,
//! `env()` and `full()` functions from this library pass through all errors returned by
//! `std::env::var()`, therefore they will also return an error if some unknown environment
//! variable is used, because `std::env::var()` returns an error in this case:
//!
//! ```
//! use std::env;
//!
//! // make sure that the variable indeed does not exist
//! env::remove_var("MOST_LIKELY_NONEXISTING_VAR");
//!
//! assert_eq!(
//!     shellexpand::env("$MOST_LIKELY_NONEXISTING_VAR"),
//!     Err(shellexpand::LookupError {
//!         name: "MOST_LIKELY_NONEXISTING_VAR".into(),
//!         cause: env::VarError::NotPresent
//!     })
//! );
//! ```
//!
//! The author thinks that this approach is more useful than just substituting an empty string
//! (like, for example, Go does with its [os.ExpandEnv](https://golang.org/pkg/os/#ExpandEnv)
//! function), but if you do need `os.ExpandEnv`-like behavior, it is fairly easy to get one:
//!
//! ```
//! use std::env;
//! use std::borrow::Cow;
//!
//! fn context(s: &str) -> Result<Option<Cow<'static, str>>, env::VarError> {
//!     match env::var(s) {
//!         Ok(value) => Ok(Some(value.into())),
//!         Err(env::VarError::NotPresent) => Ok(Some("".into())),
//!         Err(e) => Err(e)
//!     }
//! }
//!
//! // make sure that the variable indeed does not exist
//! env::remove_var("MOST_LIKELY_NONEXISTING_VAR");
//!
//! assert_eq!(
//!     shellexpand::env_with_context("a${MOST_LIKELY_NOEXISTING_VAR}b", context).unwrap(),
//!     "ab"
//! );
//! ```
//!
//! The above example also demonstrates the flexibility of context function signatures: the context
//! function may return anything which can be `AsRef`ed into a string slice.

use std::borrow::Cow;
use std::env::VarError;
use std::error::Error;
use std::fmt;
use std::path::Path;

/// Performs both tilde and environment expansion using the provided contexts.
///
/// `home_dir` and `context` are contexts for tilde expansion and environment expansion,
/// respectively. See `env_with_context()` and `tilde_with_context()` for more details on
/// them.
///
/// Unfortunately, expanding both `~` and `$VAR`s at the same time is not that simple. First,
/// this function has to track ownership of the data. Since all functions in this crate
/// return `Cow<str>`, this function takes some precautions in order not to allocate more than
/// necessary. In particular, if the input string contains neither tilde nor `$`-vars, this
/// function will perform no allocations.
///
/// Second, if the input string starts with a variable, and the value of this variable starts
/// with tilde, the naive approach may result into expansion of this tilde. This function
/// avoids this.
///
/// # Examples
///
/// ```
/// use std::path::{PathBuf, Path};
/// use std::borrow::Cow;
///
/// fn home_dir() -> Option<PathBuf> { Some(Path::new("/home/user").into()) }
///
/// fn get_env(name: &str) -> Result<Option<&'static str>, &'static str> {
///     match name {
///         "A" => Ok(Some("a value")),
///         "B" => Ok(Some("b value")),
///         "T" => Ok(Some("~")),
///         "E" => Err("some error"),
///         _ => Ok(None)
///     }
/// }
///
/// // Performs both tilde and environment expansions
/// assert_eq!(
///     shellexpand::full_with_context("~/$A/$B", home_dir, get_env).unwrap(),
///     "/home/user/a value/b value"
/// );
///
/// // Errors from environment expansion are propagated to the result
/// assert_eq!(
///     shellexpand::full_with_context("~/$E/something", home_dir, get_env),
///     Err(shellexpand::LookupError {
///         name: "E".into(),
///         cause: "some error"
///     })
/// );
/// 
/// // Input without starting tilde and without variables does not cause allocations
/// let s = shellexpand::full_with_context("some/path", home_dir, get_env);
/// match s {
///     Ok(Cow::Borrowed(s)) => assert_eq!(s, "some/path"),
///     _ => unreachable!("the above variant is always valid")
/// }
///
/// // Input with a tilde inside a variable in the beginning of the string does not cause tilde
/// // expansion
/// assert_eq!(
///     shellexpand::full_with_context("$T/$A/$B", home_dir, get_env).unwrap(),
///     "~/a value/b value"
/// );
/// ```
pub fn full_with_context<SI: ?Sized, CO, C, E, P, HD>(input: &SI, home_dir: HD, context: C) -> Result<Cow<str>, LookupError<E>>
    where SI: AsRef<str>,
          CO: AsRef<str>,
          C: FnMut(&str) -> Result<Option<CO>, E>,
          P: AsRef<Path>,
          HD: FnMut() -> Option<P>
{
    env_with_context(input, context).map(|r| match r {
        // variable expansion did not modify the original string, so we can apply tilde expansion
        // directly
        Cow::Borrowed(s) => tilde_with_context(s, home_dir),
        Cow::Owned(s) => {
            // if the original string does not start with a tilde but the processed one does,
            // then the tilde is contained in one of variables and should not be expanded
            if !input.as_ref().starts_with("~") && s.starts_with("~") {
                // return as is
                s.into()
            } else {
                if let Cow::Owned(s) = tilde_with_context(&s, home_dir) {
                    s.into()
                } else {
                    s.into()
                }
            }
        }
    })
}

/// Same as `full_with_context()`, but forbids variable lookup function to return errors.
///
/// This function also performs full shell-like expansion, but it uses
/// `env_with_context_no_errors()` for environment expansion whose context lookup function returns
/// just `Option<CO>` instead of `Result<Option<CO>, E>`. Therefore, the function itself also
/// returns just `Cow<str>` instead of `Result<Cow<str>, LookupError<E>>`. Otherwise it is
/// identical to `full_with_context()`.
///
/// # Examples
///
/// ```
/// use std::path::{PathBuf, Path};
/// use std::borrow::Cow;
///
/// fn home_dir() -> Option<PathBuf> { Some(Path::new("/home/user").into()) }
///
/// fn get_env(name: &str) -> Option<&'static str> {
///     match name {
///         "A" => Some("a value"),
///         "B" => Some("b value"),
///         "T" => Some("~"),
///         _ => None
///     }
/// }
///
/// // Performs both tilde and environment expansions
/// assert_eq!(
///     shellexpand::full_with_context_no_errors("~/$A/$B", home_dir, get_env),
///     "/home/user/a value/b value"
/// );
///
/// // Input without starting tilde and without variables does not cause allocations
/// let s = shellexpand::full_with_context_no_errors("some/path", home_dir, get_env);
/// match s {
///     Cow::Borrowed(s) => assert_eq!(s, "some/path"),
///     _ => unreachable!("the above variant is always valid")
/// }
///
/// // Input with a tilde inside a variable in the beginning of the string does not cause tilde
/// // expansion
/// assert_eq!(
///     shellexpand::full_with_context_no_errors("$T/$A/$B", home_dir, get_env),
///     "~/a value/b value"
/// );
/// ```
#[inline]
pub fn full_with_context_no_errors<SI: ?Sized, CO, C, P, HD>(input: &SI, home_dir: HD, mut context: C) -> Cow<str>
    where SI: AsRef<str>,
          CO: AsRef<str>,
          C: FnMut(&str) -> Option<CO>,
          P: AsRef<Path>,
          HD: FnMut() -> Option<P>
{
    match full_with_context(input, home_dir, move |s| Ok::<Option<CO>, ()>(context(s))) {
        Ok(result) => result,
        Err(_) => unreachable!()
    }
}

#[inline]
pub fn full<SI: ?Sized>(input: &SI) -> Result<Cow<str>, LookupError<VarError>>
    where SI: AsRef<str>
{
    full_with_context(input, std::env::home_dir, |s| std::env::var(s).map(Some))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupError<E> {
    pub name: String,
    pub cause: E
}

impl<E: fmt::Display> fmt::Display for LookupError<E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "error looking key '{}' up: {}", self.name, self.cause)
    }
}

impl<E: Error> Error for LookupError<E> {
    fn description(&self) -> &str { "lookup error" }
    fn cause(&self) -> Option<&Error> { Some(&self.cause) }
}

macro_rules! try_lookup {
    ($name:expr, $e:expr) => {
        match $e {
            Ok(s) => s,
            Err(e) => return Err(LookupError { name: $name.into(), cause: e })
        }
    }
}

fn is_valid_var_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn env_with_context<SI: ?Sized, CO, C, E>(input: &SI, mut context: C) -> Result<Cow<str>, LookupError<E>>
    where SI: AsRef<str>,
          CO: AsRef<str>,
          C: FnMut(&str) -> Result<Option<CO>, E>
{
    let input_str = input.as_ref();
    if let Some(idx) = input_str.find('$') {
        let mut result = String::with_capacity(input_str.len());

        let mut input_str = input_str;
        let mut next_dollar_idx = idx;
        loop {
            result.push_str(&input_str[..next_dollar_idx]);

            input_str = &input_str[next_dollar_idx..];
            if input_str.is_empty() { break; }

            fn find_dollar(s: &str) -> usize { s.find('$').unwrap_or(s.len()) }

            let next_char = input_str[1..].chars().next();
            if next_char == Some('{') {
                match input_str.find('}') {
                    Some(closing_brace_idx) => {
                        let var_name = &input_str[2..closing_brace_idx];
                        match try_lookup!(var_name, context(var_name)) {
                            Some(var_value) => {
                                result.push_str(var_value.as_ref());
                                input_str = &input_str[closing_brace_idx+1..];
                                next_dollar_idx = find_dollar(input_str);
                            }
                            None => {
                                result.push_str(&input_str[..closing_brace_idx+1]);
                                input_str = &input_str[closing_brace_idx+1..];
                                next_dollar_idx = find_dollar(input_str);
                            }
                        }
                    }
                    None => {
                        result.push_str(&input_str[..2]);
                        input_str = &input_str[2..];
                        next_dollar_idx = find_dollar(input_str);
                    }
                }
            } else if next_char.map(is_valid_var_name_char) == Some(true) {
                let end_idx = 2 + input_str[2..]
                    .find(|c: char| !is_valid_var_name_char(c))
                    .unwrap_or(input_str.len() - 2);

                let var_name = &input_str[1..end_idx];
                match try_lookup!(var_name, context(var_name)) {
                    Some(var_value) => {
                        result.push_str(var_value.as_ref());
                        input_str = &input_str[end_idx..];
                        next_dollar_idx = find_dollar(input_str);
                    }
                    None => {
                        result.push_str(&input_str[..end_idx]);
                        input_str = &input_str[end_idx..];
                        next_dollar_idx = find_dollar(input_str);
                    }
                }
            } else {
                result.push_str("$");
                input_str = if next_char == Some('$') {
                    &input_str[2..]   // skip the next dollar for escaping
                } else {
                    &input_str[1..] 
                };
                next_dollar_idx = find_dollar(input_str);
            };
        }
        Ok(result.into())
    } else {
        Ok(input_str.into())
    }
}

#[inline]
pub fn env_with_context_no_errors<SI: ?Sized, CO, C>(input: &SI, mut context: C) -> Cow<str>
    where SI: AsRef<str>,
          CO: AsRef<str>,
          C: FnMut(&str) -> Option<CO>
{
    match env_with_context(input, move |s| Ok::<Option<CO>, ()>(context(s))) {
        Ok(value) => value,
        Err(_) => unreachable!()
    }
}

#[inline]
pub fn env<SI: ?Sized>(input: &SI) -> Result<Cow<str>, LookupError<VarError>>
    where SI: AsRef<str>
{
    env_with_context(input, |s| std::env::var(s).map(Some))
}

pub fn tilde_with_context<SI: ?Sized, P, HD>(input: &SI, mut home_dir: HD) -> Cow<str>
    where SI: AsRef<str>,
          P: AsRef<Path>,
          HD: FnMut() -> Option<P>
{
    let input_str = input.as_ref();
    if input_str.starts_with("~") {
        let input_after_tilde = &input_str[1..];
        if input_after_tilde.is_empty() || input_after_tilde.starts_with("/") {
            if let Some(hd) = home_dir() {
                let result = format!("{}{}", hd.as_ref().display(), input_after_tilde);
                result.into()
            } else {
                // home dir is not available
                input_str.into()
            }
        } else {
            // we cannot handle `~otheruser/` paths yet
            input_str.into()
        }
    } else {
        // input doesn't start with tilde
        input_str.into()
    }
}

#[inline]
pub fn tilde<SI: ?Sized>(input: &SI) -> Cow<str>
    where SI: AsRef<str>
{
    tilde_with_context(input, std::env::home_dir)
}

#[cfg(test)]
mod tilde_tests {
    use std::path::{Path, PathBuf};
    use std::env;

    use super::{tilde, tilde_with_context};

    #[test]
    fn test_with_tilde_no_hd() {
        fn hd() -> Option<PathBuf> { None }

        assert_eq!(tilde_with_context("whatever", hd), "whatever");
        assert_eq!(tilde_with_context("whatever/~", hd), "whatever/~");
        assert_eq!(tilde_with_context("~/whatever", hd), "~/whatever");
        assert_eq!(tilde_with_context("~", hd), "~");
        assert_eq!(tilde_with_context("~something", hd), "~something");
    }

    #[test]
    fn test_with_tilde() {
        fn hd() -> Option<PathBuf> { Some(Path::new("/home/dir").into()) }

        assert_eq!(tilde_with_context("whatever/path", hd), "whatever/path");
        assert_eq!(tilde_with_context("whatever/~/path", hd), "whatever/~/path");
        assert_eq!(tilde_with_context("~", hd), "/home/dir");
        assert_eq!(tilde_with_context("~/path", hd), "/home/dir/path");
        assert_eq!(tilde_with_context("~whatever/path", hd), "~whatever/path");
    }

    #[test]
    fn test_global_tilde() {
        match env::home_dir() {
            Some(hd) => assert_eq!(tilde("~/something"), format!("{}/something", hd.display())),
            None => assert_eq!(tilde("~/something"), "~/something")
        }
    }
}

#[cfg(test)]
mod env_test {
    use std;

    use super::{env, env_with_context, LookupError};

    macro_rules! table {
        ($env:expr, unwrap, $($source:expr => $target:expr),+) => {
            $(
                assert_eq!(env_with_context($source, $env).unwrap(), $target);
            )+
        };
        ($env:expr, error, $($source:expr => $name:expr),+) => {
            $(
                assert_eq!(env_with_context($source, $env), Err(LookupError {
                    name: $name.into(),
                    cause: ()
                }));
            )+
        }
    }

    #[test]
    fn test_empty_env() {
        fn e(_: &str) -> Result<Option<String>, ()> { Ok(None) }

        table! { e, unwrap,
            "whatever/path"        => "whatever/path",
            "$VAR/whatever/path"   => "$VAR/whatever/path",
            "whatever/$VAR/path"   => "whatever/$VAR/path",
            "whatever/path/$VAR"   => "whatever/path/$VAR",
            "${VAR}/whatever/path" => "${VAR}/whatever/path",
            "whatever/${VAR}path"  => "whatever/${VAR}path",
            "whatever/path/${VAR}" => "whatever/path/${VAR}",
            "${}/whatever/path"    => "${}/whatever/path",
            "whatever/${}path"     => "whatever/${}path",
            "whatever/path/${}"    => "whatever/path/${}",
            "$/whatever/path"      => "$/whatever/path",
            "whatever/$path"       => "whatever/$path",
            "whatever/path/$"      => "whatever/path/$",
            "$$/whatever/path"     => "$/whatever/path",
            "whatever/$$path"      => "whatever/$path",
            "whatever/path/$$"     => "whatever/path/$",
            "$A$B$C"               => "$A$B$C",
            "$A_B_C"               => "$A_B_C"
        };
    }

    #[test]
    fn test_error_env() {
        fn e(_: &str) -> Result<Option<String>, ()> { Err(()) }

        table! { e, unwrap,
            "whatever/path" => "whatever/path",
            // check that escaped $ does nothing
            "whatever/$/path" => "whatever/$/path",
            "whatever/path$" => "whatever/path$",
            "whatever/$$path" => "whatever/$path"
        };

        table! { e, error,
            "$VAR/something" => "VAR",
            "${VAR}/something" => "VAR",
            "whatever/${VAR}/something" => "VAR",
            "whatever/${VAR}" => "VAR",
            "whatever/$VAR/something" => "VAR",
            "whatever/$VARsomething" => "VARsomething",
            "whatever/$VAR" => "VAR",
            "whatever/$VAR_VAR_VAR" => "VAR_VAR_VAR"
        };
    }

    #[test]
    fn test_regular_env() {
        fn e(s: &str) -> Result<Option<&'static str>, ()> {
            match s {
                "VAR" => Ok(Some("value")),
                "a_b" => Ok(Some("X_Y")),
                "EMPTY" => Ok(Some("")),
                "ERR" => Err(()),
                _ => Ok(None)
            }
        }

        table! { e, unwrap,
            // no variables
            "whatever/path" => "whatever/path",

            // empty string
            "" => "",

            // existing variable without braces in various positions
            "$VAR/whatever/path" => "value/whatever/path",
            "whatever/$VAR/path" => "whatever/value/path",
            "whatever/path/$VAR" => "whatever/path/value",
            "whatever/$VARpath" => "whatever/$VARpath",
            "$VAR$VAR/whatever" => "valuevalue/whatever",
            "/whatever$VAR$VAR" => "/whatevervaluevalue",
            "$VAR $VAR" => "value value",
            "$a_b" => "X_Y",
            "$a_b$VAR" => "X_Yvalue",

            // existing variable with braces in various positions
            "${VAR}/whatever/path" => "value/whatever/path",
            "whatever/${VAR}/path" => "whatever/value/path",
            "whatever/path/${VAR}" => "whatever/path/value",
            "whatever/${VAR}path" => "whatever/valuepath",
            "${VAR}${VAR}/whatever" => "valuevalue/whatever",
            "/whatever${VAR}${VAR}" => "/whatevervaluevalue",
            "${VAR} ${VAR}" => "value value",
            "${VAR}$VAR" => "valuevalue",

            // empty variable in various positions
            "${EMPTY}/whatever/path" => "/whatever/path",
            "whatever/${EMPTY}/path" => "whatever//path",
            "whatever/path/${EMPTY}" => "whatever/path/"
        };

        table! { e, error,
            "$ERR" => "ERR",
            "${ERR}" => "ERR"
        };
    }

    #[test]
    fn test_global_env() {
        match std::env::var("PATH") {
            Ok(value) => assert_eq!(env("x/$PATH/x").unwrap(), format!("x/{}/x", value)),
            Err(e) => assert_eq!(env("x/$PATH/x"), Err(LookupError {
                name: "PATH".into(),
                cause: e
            }))
        }
        match std::env::var("SOMETHING_DEFINITELY_NONEXISTING") {
            Ok(value) => assert_eq!(
                env("x/$SOMETHING_DEFINITELY_NONEXISTING/x").unwrap(),
                format!("x/{}/x", value)
            ),
            Err(e) => assert_eq!(env("x/$SOMETHING_DEFINITELY_NONEXISTING/x"), Err(LookupError {
                name: "SOMETHING_DEFINITELY_NONEXISTING".into(),
                cause: e
            }))
        }
    }
}

#[cfg(test)]
mod full_tests {
    use std::path::{PathBuf, Path};

    use super::full_with_context;

    #[test]
    fn test_quirks() {
        fn hd() -> Option<PathBuf> { Some(Path::new("$VAR").into()) }
        fn env(s: &str) -> Result<Option<&'static str>, ()> {
            match s {
                "VAR" => Ok(Some("value")),
                "SVAR" => Ok(Some("/value")),
                "TILDE" => Ok(Some("~")),
                _ => Ok(None)
            }
        }

        // any variable-like sequence in ~ expansion should not trigger variable expansion
        assert_eq!(full_with_context("~/something/$VAR", hd, env).unwrap(), "$VAR/something/value");

        // variable just after tilde should be substituted first and trigger regular tilde
        // expansion
        assert_eq!(full_with_context("~$VAR", hd, env).unwrap(), "~value");
        assert_eq!(full_with_context("~$SVAR", hd, env).unwrap(), "$VAR/value");

        // variable expanded into a tilde in the beginning should not trigger tilde expansion
        assert_eq!(full_with_context("$TILDE/whatever", hd, env).unwrap(), "~/whatever");
        assert_eq!(full_with_context("${TILDE}whatever", hd, env).unwrap(), "~whatever");
        assert_eq!(full_with_context("$TILDE", hd, env).unwrap(), "~");
    }
}
