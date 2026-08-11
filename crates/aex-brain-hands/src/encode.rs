//! Catalogue tool names, encoded as the guest operations that actually do the work.
//!
//! The guest implements a closed **operation** vocabulary (`Exec`, `ReadFile`,
//! `ListDir`, `StatPath`, `Search`, `WriteFile`, `EditFile`, `ProcessStatus`,
//! `ProcessStop`).
//! The model sees a different vocabulary: the **catalogue** rows in
//! `aex_brain_tool_catalog::catalog`. Nothing between the two translated, so every
//! Hands-routed call arrived at the guest as `RegisteredTool` and came back
//! `capability_unavailable`. This module is that translation, and it is the only
//! place the two vocabularies meet.
//!
//! # The map
//!
//! | catalogue row | guest operation | notes |
//! | --- | --- | --- |
//! | `read_file` | `ReadFile` | `offsetBytes`/`maxBytes` become the inclusive wire range; a line window has no wire field and is refused |
//! | `list_dir` | `ListDir` | no path means the guest root, which is what a bare `ls` is |
//! | `glob` | `Search` with [`SearchPattern::Glob`] | |
//! | `grep` | `Search` with [`SearchPattern::Regex`] | the guest matches a regex as a literal (HS-08): narrower than promised, never wider |
//! | `write_file` | `WriteFile` | bounded inline UTF-8 bytes, atomically published by the guest |
//! | `edit_file` | `EditFile` | one exact-replacement hunk, guarded by `expectedRevision` |
//! | `bash` | `Exec` | `command` is handed to [`COMMAND_SHELL`] |
//! | `git` | `Exec` via [`crate::operation::git`] | |
//! | `install_packages` | `Exec` via [`crate::operation::package_install`] | `apt` selects the image's real manager, `dnf` |
//! | `process_output` | `ProcessStatus` | the wire arm *is* the bounded output window |
//! | `process_stop` | `ProcessStop` | |
//! | `apply_patch`, `run_code`, `process_status`, `browser_*` | none | [`HandsTool::unserved_reason`] says why, per row |
//!
//! `StatPath` is implemented by the guest and no catalogue row asks for it. That
//! is recorded rather than papered over: the gap is a missing catalogue row, not
//! a missing guest capability.
//!
//! # What a row with no counterpart does
//!
//! Nothing silent. [`HandsTool::unserved_reason`] returns the exact reason, the
//! executor reports the row as unsupported, and readiness therefore never
//! advertises it — the model is not offered a tool that could only fail. Adding a
//! catalogue row without deciding its arm breaks
//! `every_hands_routed_catalogue_row_has_an_arm`; adding a [`HandsTool`] variant
//! without an arm does not compile.
//!
//! # What this module does not fix
//!
//! The guest returns each operation's body as **text** — a listing is lines, a
//! stat is one CSV row, an exec is its captured output. The catalogue's
//! `result_schema` advertises structured JSON for the same rows. Nothing
//! validates a result against that schema, so the text reaches the model intact,
//! but the advertised shape and the delivered shape still differ. Result shaping
//! is a separate seam and is deliberately untouched here.

use std::collections::BTreeMap;

use aex_hands_protocol::operation::{
    ByteRangeRequest, EnvName, EnvValue, FileMode, GuestPath, GuestPathError, GuestProcessId,
    GuestRoot, OperationRequest, Patch, PatchHunk, SearchPattern, StopSignal,
};
use aex_wire::ids::{ContentHash, Uuid7};
use aex_wire::types::DecimalU128;
use serde_json::{Map, Value};

use crate::operation::{ConstructError, ConstructedCommand, PackageManager, git, package_install};

/// The program a `bash` `command` string is handed to.
///
/// The catalogue offers `command` as one shell line. The only honest way to run
/// one is to run a shell. `bash` is in the pinned image
/// package set, and the customer is already root inside the guest, so this adds
/// no reach — refusing it would only mean the model cannot run `ls`.
pub const COMMAND_SHELL: [&str; 2] = ["bash", "-lc"];

/// How many directory entries a listing returns when the call names no limit.
pub const DEFAULT_LIST_LIMIT: u32 = 1_000;

/// How many matches a search returns when the call names no limit.
pub const DEFAULT_SEARCH_LIMIT: u32 = 1_000;

/// How many output bytes a process read returns when the call names no window.
pub const DEFAULT_OUTPUT_WINDOW_BYTES: u32 = 65_536;

/// One Hands-routed catalogue row, as a closed set.
///
/// Closed on purpose: every encoder match over this type is exhaustive, so a new
/// tool cannot be added without deciding what it becomes on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HandsTool {
    /// `read_file`.
    ReadFile,
    /// `write_file`.
    WriteFile,
    /// `edit_file`.
    EditFile,
    /// `apply_patch`.
    ApplyPatch,
    /// `list_dir`.
    ListDir,
    /// `glob`.
    Glob,
    /// `grep`.
    Grep,
    /// `bash`.
    Bash,
    /// `run_code`.
    RunCode,
    /// `git`.
    Git,
    /// `install_packages`.
    InstallPackages,
    /// `process_status`.
    ProcessStatus,
    /// `process_output`.
    ProcessOutput,
    /// `process_stop`.
    ProcessStop,
    /// `browser_launch`.
    BrowserLaunch,
    /// `browser_control`.
    BrowserControl,
    /// `browser_status`.
    BrowserStatus,
    /// `browser_result`.
    BrowserResult,
}

impl HandsTool {
    /// Every Hands-routed catalogue row this encoder knows.
    pub const ALL: [Self; 18] = [
        Self::ApplyPatch,
        Self::BrowserControl,
        Self::BrowserLaunch,
        Self::BrowserResult,
        Self::BrowserStatus,
        Self::EditFile,
        Self::Git,
        Self::Glob,
        Self::Grep,
        Self::InstallPackages,
        Self::ListDir,
        Self::ProcessOutput,
        Self::ProcessStatus,
        Self::ProcessStop,
        Self::ReadFile,
        Self::RunCode,
        Self::Bash,
        Self::WriteFile,
    ];

    /// The catalogue name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadFile => "read_file",
            Self::WriteFile => "write_file",
            Self::EditFile => "edit_file",
            Self::ApplyPatch => "apply_patch",
            Self::ListDir => "list_dir",
            Self::Glob => "glob",
            Self::Grep => "grep",
            Self::Bash => "bash",
            Self::RunCode => "run_code",
            Self::Git => "git",
            Self::InstallPackages => "install_packages",
            Self::ProcessStatus => "process_status",
            Self::ProcessOutput => "process_output",
            Self::ProcessStop => "process_stop",
            Self::BrowserLaunch => "browser_launch",
            Self::BrowserControl => "browser_control",
            Self::BrowserStatus => "browser_status",
            Self::BrowserResult => "browser_result",
        }
    }

    /// The row a catalogue name selects, when this encoder knows one.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.as_str() == name)
    }

    /// Why the guest cannot complete this row today, or `None` when it can.
    ///
    /// Every reason names the concrete thing that is missing, so it can be
    /// checked rather than believed.
    #[must_use]
    pub const fn unserved_reason(self) -> Option<&'static str> {
        match self {
            Self::ReadFile
            | Self::WriteFile
            | Self::EditFile
            | Self::ListDir
            | Self::Glob
            | Self::Grep
            | Self::Bash
            | Self::Git
            | Self::InstallPackages
            | Self::ProcessOutput
            | Self::ProcessStop => None,
            Self::ApplyPatch => Some(
                "the wire edit is exact-replacement hunks and nothing on the Brain side parses a \
                 unified diff into them",
            ),
            Self::RunCode => Some(
                "`code_run` builds the argv for a body file under `<root>/.aex/code-exec`, and \
                 no encoder materializes that private body before dispatch; `bash` runs an \
                 interpreter today",
            ),
            Self::ProcessStatus => Some(
                "the guest's process arm returns a bounded output window, not the lifecycle \
                 state this row advertises; `process_output` is the half that exists",
            ),
            Self::BrowserLaunch
            | Self::BrowserControl
            | Self::BrowserStatus
            | Self::BrowserResult => Some(
                "the guest image carries no headless browser, so the browser capability ships \
                 disabled",
            ),
        }
    }

    /// Whether the guest can complete this row today.
    #[must_use]
    pub const fn is_served(self) -> bool {
        self.unserved_reason().is_none()
    }

    /// Encodes one already schema-validated argument document.
    ///
    /// # Errors
    ///
    /// Returns [`ToolEncodingError`] when the row has no guest operation, when a
    /// required argument is absent or malformed, or when an argument names
    /// something the operation vocabulary cannot carry. Every one is refused
    /// before dispatch rather than dropped.
    #[allow(
        clippy::too_many_lines,
        reason = "the map between the two vocabularies is one cohesive table and is exhaustive by \
                  construction"
    )]
    pub fn encode(
        self,
        root: &GuestRoot,
        args: &Value,
    ) -> Result<EncodedOperation, ToolEncodingError> {
        let args = object(args)?;
        match self {
            Self::ReadFile => {
                let path = required_path(args, "path", root)?;
                refuse_field(
                    args,
                    "startLine",
                    "the wire read window is byte-addressed and has no line field",
                )?;
                refuse_field(
                    args,
                    "endLine",
                    "the wire read window is byte-addressed and has no line field",
                )?;
                Ok(EncodedOperation::instant(OperationRequest::ReadFile {
                    path,
                    range: byte_range(args)?,
                }))
            }
            Self::WriteFile => {
                let mode = match optional_text(args, "mode")? {
                    None | Some("0644") => FileMode::ReadWrite,
                    Some("0755") => FileMode::Executable,
                    Some(_) => {
                        return Err(ToolEncodingError::Malformed {
                            field: "mode",
                            expected: "`0644` or `0755`",
                        });
                    }
                };
                Ok(EncodedOperation::instant(OperationRequest::WriteFile {
                    path: required_path(args, "path", root)?,
                    mode,
                    content: required_text(args, "content")?.as_bytes().to_vec(),
                }))
            }
            Self::EditFile => {
                let path = required_path(args, "path", root)?;
                let expected = required_text(args, "expectedRevision")?;
                let expected =
                    ContentHash::parse(expected).map_err(|_| ToolEncodingError::Malformed {
                        field: "expectedRevision",
                        expected: "a `sha256:<64 lowercase hex>` digest",
                    })?;
                let occurrences = u32::from(!flag(args, "replaceAll")?.unwrap_or(false));
                Ok(EncodedOperation::instant(OperationRequest::EditFile {
                    path,
                    expected,
                    patch: Patch {
                        hunks: vec![PatchHunk {
                            expected: required_text(args, "oldString")?.to_owned(),
                            replacement: required_text(args, "newString")?.to_owned(),
                            occurrences,
                        }],
                    },
                }))
            }
            Self::ListDir => {
                refuse_field(
                    args,
                    "depth",
                    "the wire listing has no depth field; the guest descends a fixed depth",
                )?;
                if flag(args, "includeHidden")? == Some(false) {
                    return Err(ToolEncodingError::Unrepresentable {
                        field: "includeHidden",
                        reason: "the guest's listing always reports hidden entries and the wire \
                                 has no switch to exclude them",
                    });
                }
                Ok(EncodedOperation::instant(OperationRequest::ListDir {
                    path: optional_path(args, "path", root)?,
                    recursive: flag(args, "recursive")?.unwrap_or(false),
                    limit: bounded_u32(args, "limit")?.unwrap_or(DEFAULT_LIST_LIMIT),
                    // Always the first page. The wire carries a cursor because
                    // `session_files_live_list` pages a live workspace over
                    // HTTP, but the catalogue's `list_dir` publishes no cursor
                    // argument, so there is nothing here to read one from.
                    // Synthesising one would advertise paging the model cannot
                    // ask for and would move `BUILTIN_CATALOG_DIGEST`.
                    after: None,
                }))
            }
            Self::Glob => {
                let pattern = SearchPattern::Glob {
                    glob: required_text(args, "pattern")?.to_owned(),
                };
                search(args, root, pattern)
            }
            Self::Grep => {
                refuse_field(
                    args,
                    "glob",
                    "the wire search carries one pattern; a path filter beside the content \
                     pattern has no wire field",
                )?;
                if bounded_u32(args, "contextLines")?.is_some_and(|lines| lines > 0) {
                    return Err(ToolEncodingError::Unrepresentable {
                        field: "contextLines",
                        reason: "the wire search returns matching lines and has no context window",
                    });
                }
                let pattern = SearchPattern::Regex {
                    expression: required_text(args, "pattern")?.to_owned(),
                    case_sensitive: !flag(args, "ignoreCase")?.unwrap_or(false),
                };
                search(args, root, pattern)
            }
            Self::Bash => {
                refuse_field(
                    args,
                    "stdin",
                    "the wire exec takes stdin as a content reference and the guest holds no \
                     credential to resolve one",
                )?;
                let command = required_text(args, "command")?;
                let vector = COMMAND_SHELL
                    .iter()
                    .map(|word| (*word).to_owned())
                    .chain(std::iter::once(command.to_owned()))
                    .collect();
                Ok(EncodedOperation {
                    request: OperationRequest::Exec {
                        argv: vector,
                        cwd: optional_path(args, "cwd", root)?,
                        env: environment(args, "env")?,
                        stdin: None,
                    },
                    max_wall_ms: wall_bound(args)?,
                })
            }
            Self::Git => {
                let built = git(&required_strings(args, "args")?, &BTreeMap::new())?;
                Ok(exec(
                    built,
                    optional_path(args, "cwd", root)?,
                    wall_bound(args)?,
                ))
            }
            Self::InstallPackages => {
                let manager = package_manager(args)?;
                let built = package_install(
                    manager,
                    &package_specs(args, manager)?,
                    &[],
                    &BTreeMap::new(),
                )?;
                Ok(exec(built, root_path(root)?, wall_bound(args)?))
            }
            Self::ProcessOutput => {
                let max_bytes = bounded_u32(args, "maxBytes")?
                    .unwrap_or(DEFAULT_OUTPUT_WINDOW_BYTES)
                    .min(OperationRequest::MAX_OUTPUT_WINDOW_BYTES);
                Ok(EncodedOperation::instant(OperationRequest::ProcessStatus {
                    process: guest_process(args)?,
                    from_offset: decimal(args, "fromOffset")?.unwrap_or(0),
                    max_bytes,
                }))
            }
            Self::ProcessStop => {
                let signal = match optional_text(args, "signal")? {
                    None | Some("term") => StopSignal::Term,
                    Some("kill") => StopSignal::Kill,
                    Some(_) => {
                        return Err(ToolEncodingError::Malformed {
                            field: "signal",
                            expected: "`term` or `kill`",
                        });
                    }
                };
                Ok(EncodedOperation::instant(OperationRequest::ProcessStop {
                    process: guest_process(args)?,
                    signal,
                }))
            }
            Self::ApplyPatch
            | Self::RunCode
            | Self::ProcessStatus
            | Self::BrowserLaunch
            | Self::BrowserControl
            | Self::BrowserStatus
            | Self::BrowserResult => Err(ToolEncodingError::Unserved {
                tool: self.as_str(),
                reason: self
                    .unserved_reason()
                    .unwrap_or("this row has no guest operation"),
            }),
        }
    }
}

/// One encoded operation and the wall bound its arguments asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedOperation {
    /// What the guest is asked to do.
    pub request: OperationRequest,
    /// The wall bound the arguments named, when they named one.
    ///
    /// Only ever **narrows** the route's pinned timeout. A call that asks for
    /// five seconds and silently gets ten minutes is the kind of quiet
    /// difference this field exists to remove.
    pub max_wall_ms: Option<u64>,
}

impl EncodedOperation {
    /// An operation whose bound is the route's, because its arguments named none.
    #[must_use]
    pub const fn instant(request: OperationRequest) -> Self {
        Self {
            request,
            max_wall_ms: None,
        }
    }
}

/// Why a model tool call could not become a guest operation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolEncodingError {
    /// The catalogue row has no guest operation at all.
    #[error("`{tool}` has no guest operation: {reason}")]
    Unserved {
        /// The catalogue name.
        tool: &'static str,
        /// Exactly what is missing.
        reason: &'static str,
    },
    /// A required argument was absent.
    #[error("`{field}` is required")]
    Missing {
        /// Which argument.
        field: &'static str,
    },
    /// An argument was the wrong shape.
    #[error("`{field}` is not {expected}")]
    Malformed {
        /// Which argument.
        field: &'static str,
        /// What it should have been.
        expected: &'static str,
    },
    /// An argument named something the operation vocabulary cannot carry.
    #[error("`{field}` has no counterpart in the guest operation vocabulary: {reason}")]
    Unrepresentable {
        /// Which argument.
        field: &'static str,
        /// Why the wire cannot carry it.
        reason: &'static str,
    },
    /// A path argument left the guest root.
    #[error("`{field}` is not a path inside the guest root: {source}")]
    Path {
        /// Which argument.
        field: &'static str,
        /// The containment failure.
        source: GuestPathError,
    },
    /// A Brain-side argv constructor refused the arguments.
    #[error(transparent)]
    Construct(#[from] ConstructError),
}

fn object(args: &Value) -> Result<&Map<String, Value>, ToolEncodingError> {
    args.as_object().ok_or(ToolEncodingError::Malformed {
        field: "arguments",
        expected: "a JSON object",
    })
}

fn optional_text<'a>(
    args: &'a Map<String, Value>,
    field: &'static str,
) -> Result<Option<&'a str>, ToolEncodingError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or(ToolEncodingError::Malformed {
                field,
                expected: "a string",
            }),
    }
}

fn required_text<'a>(
    args: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, ToolEncodingError> {
    optional_text(args, field)?.ok_or(ToolEncodingError::Missing { field })
}

fn flag(args: &Map<String, Value>, field: &'static str) -> Result<Option<bool>, ToolEncodingError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or(ToolEncodingError::Malformed {
                field,
                expected: "a boolean",
            }),
    }
}

fn count(args: &Map<String, Value>, field: &'static str) -> Result<Option<u64>, ToolEncodingError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or(ToolEncodingError::Malformed {
                field,
                expected: "a non-negative integer",
            }),
    }
}

fn bounded_u32(
    args: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<u32>, ToolEncodingError> {
    count(args, field)?
        .map(|value| {
            u32::try_from(value).map_err(|_| ToolEncodingError::Malformed {
                field,
                expected: "an integer inside the 32-bit wire field",
            })
        })
        .transpose()
}

fn decimal(
    args: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<u64>, ToolEncodingError> {
    optional_text(args, field)?
        .map(|text| {
            text.parse::<u64>()
                .map_err(|_| ToolEncodingError::Malformed {
                    field,
                    expected: "a canonical decimal string",
                })
        })
        .transpose()
}

fn strings(
    args: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<Vec<String>>, ToolEncodingError> {
    let Some(value) = args.get(field).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let items = value.as_array().ok_or(ToolEncodingError::Malformed {
        field,
        expected: "an array of strings",
    })?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or(ToolEncodingError::Malformed {
                    field,
                    expected: "an array of strings",
                })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn required_strings(
    args: &Map<String, Value>,
    field: &'static str,
) -> Result<Vec<String>, ToolEncodingError> {
    strings(args, field)?.ok_or(ToolEncodingError::Missing { field })
}

/// Refuses an argument the operation vocabulary has no field for.
///
/// Present and non-null is a refusal. Serving the call without it would answer a
/// different question than the model asked.
fn refuse_field(
    args: &Map<String, Value>,
    field: &'static str,
    reason: &'static str,
) -> Result<(), ToolEncodingError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(()),
        Some(_) => Err(ToolEncodingError::Unrepresentable { field, reason }),
    }
}

fn root_path(root: &GuestRoot) -> Result<GuestPath, ToolEncodingError> {
    GuestPath::parse(root, &root.0).map_err(|source| ToolEncodingError::Path {
        field: "path",
        source,
    })
}

fn optional_path(
    args: &Map<String, Value>,
    field: &'static str,
    root: &GuestRoot,
) -> Result<GuestPath, ToolEncodingError> {
    match optional_text(args, field)?.filter(|text| !text.is_empty()) {
        None => root_path(root),
        Some(text) => {
            GuestPath::parse(root, text).map_err(|source| ToolEncodingError::Path { field, source })
        }
    }
}

fn required_path(
    args: &Map<String, Value>,
    field: &'static str,
    root: &GuestRoot,
) -> Result<GuestPath, ToolEncodingError> {
    let text = required_text(args, field)?;
    GuestPath::parse(root, text).map_err(|source| ToolEncodingError::Path { field, source })
}

fn byte_range(args: &Map<String, Value>) -> Result<Option<ByteRangeRequest>, ToolEncodingError> {
    let offset = count(args, "offsetBytes")?;
    let Some(max_bytes) = count(args, "maxBytes")? else {
        if offset.is_some() {
            return Err(ToolEncodingError::Unrepresentable {
                field: "offsetBytes",
                reason: "the wire read range is inclusive at both ends, so an open-ended read has \
                         no wire spelling; name `maxBytes` as well",
            });
        }
        return Ok(None);
    };
    if max_bytes == 0 {
        return Err(ToolEncodingError::Malformed {
            field: "maxBytes",
            expected: "at least one byte",
        });
    }
    let start = u128::from(offset.unwrap_or(0));
    Ok(Some(ByteRangeRequest {
        start: DecimalU128::new(start),
        end_inclusive: DecimalU128::new(start.saturating_add(u128::from(max_bytes) - 1)),
    }))
}

fn search(
    args: &Map<String, Value>,
    root: &GuestRoot,
    pattern: SearchPattern,
) -> Result<EncodedOperation, ToolEncodingError> {
    if !pattern.is_bounded() {
        return Err(ToolEncodingError::Malformed {
            field: "pattern",
            expected: "a pattern inside the wire search bound",
        });
    }
    Ok(EncodedOperation::instant(OperationRequest::Search {
        root: optional_path(args, "path", root)?,
        pattern,
        limit: bounded_u32(args, "limit")?.unwrap_or(DEFAULT_SEARCH_LIMIT),
    }))
}

fn environment(
    args: &Map<String, Value>,
    field: &'static str,
) -> Result<Vec<(EnvName, EnvValue)>, ToolEncodingError> {
    let Some(value) = args.get(field).filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let pairs = value.as_object().ok_or(ToolEncodingError::Malformed {
        field,
        expected: "an object of string values",
    })?;
    // Sorted, because the canonical request bytes are the operation's identity
    // and a map iteration order must never be part of it.
    let sorted: BTreeMap<&String, &Value> = pairs.iter().collect();
    sorted
        .into_iter()
        .map(|(name, value)| {
            value
                .as_str()
                .map(|text| (EnvName(name.clone()), EnvValue::new(text)))
                .ok_or(ToolEncodingError::Malformed {
                    field,
                    expected: "an object of string values",
                })
        })
        .collect()
}

fn guest_process(args: &Map<String, Value>) -> Result<GuestProcessId, ToolEncodingError> {
    const FIELD: &str = "operationId";
    let text = required_text(args, FIELD)?;
    // The wire spells a process identity as the operation's bare identifier
    // suffix; the model-facing catalogue spells the same value with its `op_`
    // prefix. Decoding proves it is one identity and not a guess.
    let suffix = text.strip_prefix("op_").unwrap_or(text);
    Uuid7::decode_suffix(suffix.as_bytes()).map_err(|_| ToolEncodingError::Malformed {
        field: FIELD,
        expected: "an `op_`-prefixed operation identity",
    })?;
    Ok(GuestProcessId(suffix.to_owned()))
}

fn wall_bound(args: &Map<String, Value>) -> Result<Option<u64>, ToolEncodingError> {
    count(args, "timeoutMs")
}

fn package_manager(args: &Map<String, Value>) -> Result<PackageManager, ToolEncodingError> {
    // The catalogue names the ecosystem the way a model thinks about it. `apt`
    // selects the image's real system manager, which is `dnf` on Amazon Linux
    // 2023; there is no apt in the pinned package set and pretending otherwise
    // would produce a command that cannot run.
    match required_text(args, "ecosystem")? {
        "apt" => Ok(PackageManager::Dnf),
        "pip" => Ok(PackageManager::Pip),
        "npm" => Ok(PackageManager::Npm),
        _ => Err(ToolEncodingError::Malformed {
            field: "ecosystem",
            expected: "`apt`, `pip` or `npm`",
        }),
    }
}

fn package_specs(
    args: &Map<String, Value>,
    manager: PackageManager,
) -> Result<Vec<String>, ToolEncodingError> {
    const FIELD: &str = "packages";
    let listed = args
        .get(FIELD)
        .and_then(Value::as_array)
        .ok_or(ToolEncodingError::Missing { field: FIELD })?;
    listed
        .iter()
        .map(|entry| {
            let entry = entry.as_object().ok_or(ToolEncodingError::Malformed {
                field: FIELD,
                expected: "an array of `{name, version?}` objects",
            })?;
            let name = required_text(entry, "name")?;
            Ok(match optional_text(entry, "version")? {
                None => name.to_owned(),
                Some(version) => pinned_package(manager, name, version),
            })
        })
        .collect()
}

/// How each manager spells "this exact version".
fn pinned_package(manager: PackageManager, name: &str, version: &str) -> String {
    match manager {
        PackageManager::Dnf => format!("{name}-{version}"),
        PackageManager::Pip | PackageManager::Cargo => format!("{name}=={version}"),
        PackageManager::Npm => format!("{name}@{version}"),
    }
}

fn exec(
    built: ConstructedCommand,
    cwd: GuestPath,
    requested_wall_ms: Option<u64>,
) -> EncodedOperation {
    let env = built
        .env
        .into_iter()
        .map(|(name, value)| (EnvName(name), EnvValue::new(value)))
        .collect();
    EncodedOperation {
        request: OperationRequest::Exec {
            argv: built.argv,
            cwd,
            env,
            stdin: None,
        },
        max_wall_ms: Some(requested_wall_ms.map_or(built.max_wall_ms, |requested| {
            requested.min(built.max_wall_ms)
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::{COMMAND_SHELL, DEFAULT_LIST_LIMIT, HandsTool, ToolEncodingError};
    use aex_brain_tool_catalog::wire_pending::ExecutorRoute;
    use aex_hands_protocol::operation::{GuestRoot, OperationRequest, SearchPattern, StopSignal};
    use serde_json::json;

    fn root() -> GuestRoot {
        aex_runtime_control::generation::guest_root()
    }

    fn encode(name: &str, args: &serde_json::Value) -> OperationRequest {
        HandsTool::parse(name)
            .expect("a known catalogue row")
            .encode(&root(), args)
            .expect("the arguments encode")
            .request
    }

    #[test]
    fn every_mvp_hands_routed_catalogue_row_has_a_served_arm() {
        let entries =
            aex_brain_tool_catalog::catalog::builtin_entries().expect("the catalogue builds");
        let routed = entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.descriptor.route,
                    ExecutorRoute::HandsFilesystem
                        | ExecutorRoute::HandsDevelopment
                        | ExecutorRoute::HandsBrowser
                )
            })
            .map(|entry| entry.descriptor.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            routed,
            ["bash", "edit_file", "read_file", "write_file"],
            "a Hands-routed catalogue row was added or removed without deciding its guest \
             operation"
        );
        assert!(
            routed
                .iter()
                .all(|name| { HandsTool::parse(name).is_some_and(HandsTool::is_served) })
        );
    }

    #[test]
    fn a_bare_listing_is_the_guest_root_and_not_a_registered_tool() {
        // This is the owner's `ls`: the model names no path at all.
        let request = encode("list_dir", &json!({}));
        let OperationRequest::ListDir {
            path,
            recursive,
            limit,
            after,
        } = request
        else {
            panic!("a listing is a ListDir, not a RegisteredTool");
        };
        assert_eq!(path.as_str(), "/workspace");
        assert!(!recursive);
        assert_eq!(limit, DEFAULT_LIST_LIMIT);
        assert!(
            after.is_none(),
            "the catalogue publishes no cursor, so a model listing starts at the beginning"
        );
    }

    #[test]
    fn a_bash_command_becomes_one_attached_exec() {
        let OperationRequest::Exec { argv, cwd, .. } =
            encode("bash", &json!({"command": "ls -la"}))
        else {
            panic!("a command is an Exec");
        };
        assert_eq!(argv, vec![COMMAND_SHELL[0], COMMAND_SHELL[1], "ls -la"]);
        assert_eq!(cwd.as_str(), "/workspace");
    }

    #[test]
    fn a_write_carries_bounded_inline_bytes_and_an_explicit_mode() {
        let OperationRequest::WriteFile {
            path,
            mode,
            content,
        } = encode(
            "write_file",
            &json!({"path": "/workspace/a.txt", "content": "hello\n", "mode": "0755"}),
        )
        else {
            panic!("a model write is a structured WriteFile operation");
        };
        assert_eq!(path.as_str(), "/workspace/a.txt");
        assert_eq!(mode, aex_hands_protocol::operation::FileMode::Executable);
        assert_eq!(content, b"hello\n");

        let OperationRequest::WriteFile { mode, .. } = encode(
            "write_file",
            &json!({"path": "/workspace/default.txt", "content": ""}),
        ) else {
            panic!("a model write is a structured WriteFile operation");
        };
        assert_eq!(mode, aex_hands_protocol::operation::FileMode::ReadWrite);

        let content = "x".repeat(500_000);
        let encoded = HandsTool::WriteFile
            .encode(
                &root(),
                &json!({"path": "/workspace/large.txt", "content": content}),
            )
            .expect("the largest catalog write encodes");
        let bytes = serde_json::to_vec(&encoded.request).expect("the operation serializes");
        assert!(
            bytes.len() + 4_096 < crate::MAX_FRAME_BYTES as usize,
            "the closed StartRequest envelope must still fit around the largest write operation"
        );
    }

    #[test]
    fn the_dead_argv_constructors_are_the_ones_that_build_git_and_installs() {
        let OperationRequest::Exec { argv, .. } = encode("git", &json!({"args": ["status"]}))
        else {
            panic!("git is an Exec");
        };
        assert_eq!(argv, vec!["git", "status"]);

        let OperationRequest::Exec { argv, .. } = encode(
            "install_packages",
            &json!({"ecosystem": "apt", "packages": [{"name": "jq"}, {"name": "ripgrep", "version": "14.1.0"}]}),
        ) else {
            panic!("an install is an Exec");
        };
        assert_eq!(
            argv,
            vec!["dnf", "install", "-y", "jq", "ripgrep-14.1.0"],
            "`apt` selects the image's real manager and a version is pinned in its own spelling"
        );
    }

    #[test]
    fn a_named_wall_bound_only_ever_narrows_the_routes_own() {
        let encoded = HandsTool::Git
            .encode(&root(), &json!({"args": ["fetch"], "timeoutMs": 5_000}))
            .expect("the arguments encode");
        assert_eq!(encoded.max_wall_ms, Some(5_000));

        let encoded = HandsTool::Git
            .encode(&root(), &json!({"args": ["fetch"], "timeoutMs": 9_000_000}))
            .expect("the arguments encode");
        assert_eq!(
            encoded.max_wall_ms,
            Some(crate::operation::TOOLCHAIN_WALL_MS),
            "a request above the tool's own bound is clamped, never granted"
        );
    }

    #[test]
    fn reads_listings_searches_and_process_reads_reach_their_own_arms() {
        assert!(matches!(
            encode("read_file", &json!({"path": "/workspace/a.txt"})),
            OperationRequest::ReadFile { range: None, .. }
        ));
        assert!(matches!(
            encode("glob", &json!({"pattern": "**/*.rs"})),
            OperationRequest::Search {
                pattern: SearchPattern::Glob { .. },
                ..
            }
        ));
        assert!(matches!(
            encode("grep", &json!({"pattern": "needle", "ignoreCase": true})),
            OperationRequest::Search {
                pattern: SearchPattern::Regex {
                    case_sensitive: false,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            encode(
                "edit_file",
                &json!({
                    "path": "/workspace/a.txt",
                    "expectedRevision": format!("sha256:{}", "0".repeat(64)),
                    "oldString": "a",
                    "newString": "b"
                })
            ),
            OperationRequest::EditFile { .. }
        ));
        let suffix = aex_wire::ids::Uuid7::compose(1, [1; 10]).encode_suffix();
        let suffix = std::str::from_utf8(&suffix)
            .expect("a base32 suffix")
            .to_owned();
        let OperationRequest::ProcessStop { process, signal } = encode(
            "process_stop",
            &json!({"operationId": format!("op_{suffix}"), "signal": "kill"}),
        ) else {
            panic!("a stop is a ProcessStop");
        };
        assert_eq!(signal, StopSignal::Kill);
        assert_eq!(
            process.0, suffix,
            "the wire names a process by the operation's bare identifier suffix"
        );
    }

    #[test]
    fn an_argument_the_wire_cannot_carry_is_refused_and_never_dropped() {
        let cases = [
            (
                "read_file",
                json!({"path": "/workspace/a.txt", "startLine": 2}),
                "startLine",
            ),
            ("list_dir", json!({"depth": 4}), "depth"),
            ("list_dir", json!({"includeHidden": false}), "includeHidden"),
            (
                "grep",
                json!({"pattern": "x", "contextLines": 3}),
                "contextLines",
            ),
            ("bash", json!({"command": "cat", "stdin": "hello"}), "stdin"),
        ];
        for (tool, args, field) in cases {
            let error = HandsTool::parse(tool)
                .expect("a known row")
                .encode(&root(), &args)
                .expect_err("an unrepresentable argument is refused");
            assert!(
                matches!(error, ToolEncodingError::Unrepresentable { field: named, .. } if named == field),
                "{tool}.{field} produced {error:?}"
            );
        }
    }

    #[test]
    fn a_path_outside_the_guest_root_never_becomes_an_operation() {
        let error = HandsTool::ReadFile
            .encode(&root(), &json!({"path": "/etc/shadow"}))
            .expect_err("containment is checked before dispatch");
        assert!(matches!(error, ToolEncodingError::Path { .. }));
    }

    #[test]
    fn every_unserved_row_says_exactly_what_is_missing() {
        for tool in HandsTool::ALL {
            let Some(reason) = tool.unserved_reason() else {
                assert!(tool.is_served());
                continue;
            };
            assert!(!tool.is_served());
            assert!(reason.len() > 32, "{tool:?} gives no checkable reason");
            let error = tool
                .encode(&root(), &json!({}))
                .expect_err("an unserved row encodes nothing");
            assert!(matches!(
                error,
                ToolEncodingError::Unserved { tool: named, .. } if named == tool.as_str()
            ));
        }
    }
}
