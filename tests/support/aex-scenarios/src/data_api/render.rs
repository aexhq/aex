//! Rendering one Data `API` statement as a `PostgreSQL` extended-protocol query.
//!
//! Two things differ between what a repository writes and what a connection
//! accepts, and both are handled here as a single pass over the statement text.
//!
//! **Named parameters.** The Data `API` binds `:name` and types the value from
//! the `TypeHint` the caller attached. `PostgreSQL` binds `$1` and types the
//! value from the *client*, which is not the same thing: bind a `uuid` column's
//! value as `text` and `WHERE id = $1` fails with `operator does not exist`. So
//! every placeholder is emitted with the cast its hint implies —
//! `:key_id` with [`TypeHint::Uuid`] becomes `$1::uuid` — which reproduces the
//! service's typing exactly and leaves an already-cast statement (`$1::uuid::uuid`)
//! harmless. A `Field::IsNull` parameter is emitted as the `NULL` keyword rather
//! than as a bind, because a Data `API` null is untyped and coerces from
//! context; a bound `text` null does not.
//!
//! **Reading the answer back.** A record has to arrive as the `Field` variants
//! `aex_rds_data::Record` accepts, positionally, and one of the columns the
//! money schema projects is `NUMERIC`, which has no exact client-side decode
//! under the features this workspace enables. So the statement is wrapped to
//! project `__aex::text` — the engine's own output function for the whole row —
//! beside the untouched column list, whose *metadata* then names each column's
//! type. Nothing is decoded from the binary protocol, so `NUMERIC` round-trips
//! by its exact decimal spelling, which is precisely what the Data `API` does.
//!
//! The wrap has three shapes because one shape does not cover the statements
//! this workspace actually contains; see [`Shape`].
//!
//! # Not this module's job
//!
//! - executing anything: [`super::transport`] owns the connection;
//! - deciding a statement is unsupported quietly. Every refusal is a
//!   [`RenderError`] naming the statement.

use aws_sdk_rdsdata::types::{Field, SqlParameter, TypeHint};

/// The alias every wrap introduces.
///
/// Long and prefixed on purpose: it shares a namespace with the statement's own
/// aliases, and a collision would change which relation a column resolves to.
const ALIAS: &str = "__aex_scenario_row";

/// One value bound to the rendered statement, in `$n` order.
///
/// The Data `API`'s vocabulary is wider than this, but only in ways that
/// collapse: a `uuid`, a `NUMERIC`, a `jsonb` document and a `text[]` carried as
/// a JSON scalar all arrive as `stringValue` and all bind as `text` under an
/// explicit cast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bound {
    /// `boolean`.
    Bool(bool),
    /// `bigint`.
    I64(i64),
    /// Anything the service transports as `stringValue`.
    Text(String),
    /// `bytea`.
    Bytes(Vec<u8>),
}

/// How a statement was wrapped so its rows can be read back as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `SELECT <alias>::text, <alias>.* FROM (<statement>) <alias>`.
    ///
    /// For a statement whose leading keyword is `SELECT`, `VALUES` or `TABLE`.
    Subquery,
    /// `WITH <alias> AS (<statement>) SELECT <alias>::text, <alias>.* FROM <alias>`.
    ///
    /// For `INSERT`/`UPDATE`/`DELETE ... RETURNING`. A data-modifying statement
    /// cannot appear in a `FROM` subquery, so it goes in the `WITH` clause,
    /// where `PostgreSQL` does admit one.
    Cte,
    /// `<the statement's own WITH list>, <alias> AS (<the rest>) SELECT ...`.
    ///
    /// For a statement that already begins with `WITH`. Nesting it inside
    /// another `WITH` would put a data-modifying `WITH` below the top level,
    /// which `PostgreSQL` refuses outright; merging into the existing list keeps
    /// every clause exactly where the engine requires it.
    Merge,
    /// Not wrapped: the statement returns no rows.
    Plain,
}

/// A statement ready for the extended protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The text to prepare.
    pub sql: String,
    /// The values to bind, in `$1..$n` order.
    pub binds: Vec<Bound>,
    /// How the statement was wrapped.
    pub shape: Shape,
    /// Whether the statement writes, which is what `rowsAffected` counts.
    pub dml: bool,
}

impl Rendered {
    /// Whether the rendered statement projects the row-text column.
    #[must_use]
    pub const fn returns_rows(&self) -> bool {
        !matches!(self.shape, Shape::Plain)
    }
}

/// Why a statement could not be rendered.
///
/// Every arm is a refusal, never a degraded rendering: a statement this module
/// does not understand must fail the case that issued it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    /// A parameter carried no value at all.
    #[error("parameter `{name}` carries no value")]
    ValuelessParameter {
        /// The parameter name.
        name: String,
    },
    /// A parameter arrived as `doubleValue`.
    #[error(
        "parameter `{name}` arrived as doubleValue; this transport has no floating-point bind path"
    )]
    DoubleParameter {
        /// The parameter name.
        name: String,
    },
    /// A parameter arrived as a `Field` variant with no `PostgreSQL` rendering.
    #[error("parameter `{name}` arrived as a Data API field variant this transport cannot bind")]
    UnbindableParameter {
        /// The parameter name.
        name: String,
    },
    /// The statement referenced a name no parameter supplied.
    #[error("statement binds `:{name}`, which no supplied parameter provides")]
    MissingParameter {
        /// The parameter name.
        name: String,
    },
    /// A quoted literal or comment never closed.
    #[error("statement has an unterminated {literal}")]
    Unterminated {
        /// Which construct.
        literal: &'static str,
    },
    /// A `WITH` statement's outer command could not be located.
    #[error("statement begins with WITH but names no outer SELECT, INSERT, UPDATE or DELETE")]
    HeadlessWith,
    /// The statement used `$` outside a literal.
    ///
    /// `$` is how this renderer spells its own placeholders and how
    /// `PostgreSQL` opens a dollar-quoted string. No committed statement in this
    /// workspace contains one, and admitting it would mean guessing which of the
    /// two a byte meant.
    #[error("statement uses `$` outside a literal, which this renderer reserves for placeholders")]
    ReservedDollar,
}

/// The command keywords a statement can lead with.
const COMMANDS: [&str; 6] = ["SELECT", "INSERT", "UPDATE", "DELETE", "VALUES", "TABLE"];

/// Renders one Data `API` statement for `PostgreSQL`.
///
/// # Errors
///
/// Returns [`RenderError`] for a parameter this transport cannot bind, a
/// parameter mismatch in either direction, or an unterminated literal.
/// A supplied parameter the statement never references is **not** an error, and
/// that tolerance is deliberate rather than lax: the Data `API` itself ignores
/// one, so refusing it here would make this transport stricter than the service
/// the statements are written for and would fail a committed path for a reason
/// production does not have. The mistake this check would have caught — a
/// misspelled name — is caught from the other direction by
/// [`RenderError::MissingParameter`].
///
/// The case that motivated it was `DENY_DEVICE_AUTHORIZATION`, which took the
/// four parameters `decide_device` binds for both decisions and referenced only
/// two. That turned out to be the defect it looks like rather than a shape
/// worth accommodating, and the statement now references all four. The
/// tolerance stays, because it describes the service rather than that
/// statement.
pub fn render(sql: &str, parameters: &[SqlParameter]) -> Result<Rendered, RenderError> {
    let scan = scan(sql, parameters)?;
    let (shape, sql, dml) = wrap(&scan)?;
    Ok(Rendered {
        sql,
        binds: scan.binds,
        shape,
        dml,
    })
}

/// The statement after placeholder substitution, plus what the scan learned.
struct Scan {
    /// The statement with `$n` placeholders.
    sql: String,
    /// The bound values in `$n` order.
    binds: Vec<Bound>,
    /// Keywords seen outside every literal at parenthesis depth zero, upper
    /// cased, with their byte offset into [`Scan::sql`].
    keywords: Vec<(String, usize)>,
}

/// Chooses the wrap and applies it, reporting whether the statement writes.
///
/// "Writes" is read off the *command* keyword, not off the presence of the word
/// `UPDATE`: `SELECT ... FOR UPDATE` is a read, and counting its rows as
/// `rowsAffected` would disagree with the service, which answers `0` for every
/// `SELECT`.
fn wrap(scan: &Scan) -> Result<(Shape, String, bool), RenderError> {
    let head = scan.keywords.first().map(|(word, _)| word.as_str());
    let returning = scan.keywords.iter().any(|(word, _)| word == "RETURNING");
    match head {
        Some("SELECT" | "VALUES" | "TABLE") => Ok((
            Shape::Subquery,
            format!(
                "SELECT {ALIAS}::text AS {ALIAS}_text, {ALIAS}.* FROM ({}) {ALIAS}",
                scan.sql
            ),
            false,
        )),
        Some("WITH") => {
            let (outer, at) = scan
                .keywords
                .iter()
                .skip(1)
                .find(|(word, _)| COMMANDS.contains(&word.as_str()))
                .ok_or(RenderError::HeadlessWith)?;
            let dml = matches!(outer.as_str(), "INSERT" | "UPDATE" | "DELETE");
            let rows = !dml || returning;
            if !rows {
                return Ok((Shape::Plain, scan.sql.clone(), dml));
            }
            let (prefix, rest) = scan.sql.split_at(*at);
            Ok((
                Shape::Merge,
                format!(
                    "{}, {ALIAS} AS ({}) SELECT {ALIAS}::text AS {ALIAS}_text, {ALIAS}.* FROM {ALIAS}",
                    prefix.trim_end(),
                    rest.trim_end()
                ),
                dml,
            ))
        }
        Some("INSERT" | "UPDATE" | "DELETE") if returning => Ok((
            Shape::Cte,
            format!(
                "WITH {ALIAS} AS ({}) SELECT {ALIAS}::text AS {ALIAS}_text, {ALIAS}.* FROM {ALIAS}",
                scan.sql
            ),
            true,
        )),
        Some("INSERT" | "UPDATE" | "DELETE") => Ok((Shape::Plain, scan.sql.clone(), true)),
        _ => Ok((Shape::Plain, scan.sql.clone(), false)),
    }
}

/// Walks the statement once, substituting placeholders and reading keywords.
#[expect(
    clippy::too_many_lines,
    reason = "one pass over one grammar; splitting it would hide which construct consumes which byte"
)]
fn scan(sql: &str, parameters: &[SqlParameter]) -> Result<Scan, RenderError> {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 32);
    let mut binds: Vec<Bound> = Vec::new();
    let mut slots: Vec<(String, usize)> = Vec::new();
    let mut keywords: Vec<(String, usize)> = Vec::new();
    let mut depth = 0_i32;
    let mut index = 0_usize;

    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                let end = sql[index..]
                    .find('\n')
                    .map_or(bytes.len(), |offset| index + offset);
                out.push_str(&sql[index..end]);
                index = end;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let mut nesting = 1_u32;
                let mut cursor = index + 2;
                while cursor < bytes.len() && nesting > 0 {
                    if bytes[cursor] == b'/' && bytes.get(cursor + 1) == Some(&b'*') {
                        nesting += 1;
                        cursor += 2;
                    } else if bytes[cursor] == b'*' && bytes.get(cursor + 1) == Some(&b'/') {
                        nesting -= 1;
                        cursor += 2;
                    } else {
                        cursor += 1;
                    }
                }
                if nesting > 0 {
                    return Err(RenderError::Unterminated {
                        literal: "block comment",
                    });
                }
                out.push_str(&sql[index..cursor]);
                index = cursor;
            }
            b'\'' | b'"' => {
                let quote = byte;
                let mut cursor = index + 1;
                loop {
                    let Some(position) = sql[cursor..].find(quote as char) else {
                        return Err(RenderError::Unterminated {
                            literal: if quote == b'\'' {
                                "string literal"
                            } else {
                                "quoted identifier"
                            },
                        });
                    };
                    cursor += position + 1;
                    if bytes.get(cursor) == Some(&quote) {
                        cursor += 1;
                    } else {
                        break;
                    }
                }
                out.push_str(&sql[index..cursor]);
                index = cursor;
            }
            b'(' => {
                depth += 1;
                out.push('(');
                index += 1;
            }
            b')' => {
                depth -= 1;
                out.push(')');
                index += 1;
            }
            b':' if bytes.get(index + 1) == Some(&b':') => {
                out.push_str("::");
                index += 2;
            }
            b':' if bytes.get(index + 1).is_some_and(|byte| is_name_byte(*byte)) => {
                let mut cursor = index + 1;
                while bytes.get(cursor).is_some_and(|byte| is_name_byte(*byte)) {
                    cursor += 1;
                }
                let name = &sql[index + 1..cursor];
                let parameter = parameters
                    .iter()
                    .find(|candidate| candidate.name() == Some(name))
                    .ok_or_else(|| RenderError::MissingParameter {
                        name: name.to_owned(),
                    })?;
                match placeholder(parameter, name, &mut binds, &mut slots)? {
                    Some(rendered) => out.push_str(&rendered),
                    None => out.push_str("NULL"),
                }
                index = cursor;
            }
            b'$' => return Err(RenderError::ReservedDollar),
            _ if byte.is_ascii_alphabetic() || byte == b'_' => {
                let mut cursor = index;
                while bytes.get(cursor).is_some_and(|byte| is_name_byte(*byte)) {
                    cursor += 1;
                }
                let word = &sql[index..cursor];
                if depth == 0 {
                    keywords.push((word.to_ascii_uppercase(), out.len()));
                }
                out.push_str(word);
                index = cursor;
            }
            _ => {
                let character = sql[index..].chars().next().unwrap_or('\0');
                out.push(character);
                index += character.len_utf8();
            }
        }
    }

    Ok(Scan {
        sql: out,
        binds,
        keywords,
    })
}

/// Whether a byte may continue an unquoted SQL name.
const fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Emits the placeholder for one parameter, reusing the slot of a repeat.
///
/// `Ok(None)` means the parameter is SQL `NULL` and is inlined as the keyword.
fn placeholder(
    parameter: &SqlParameter,
    name: &str,
    binds: &mut Vec<Bound>,
    slots: &mut Vec<(String, usize)>,
) -> Result<Option<String>, RenderError> {
    if let Some((_, ordinal)) = slots.iter().find(|(seen, _)| seen == name) {
        let cast = cast_of(parameter, name)?;
        return Ok(cast.map(|cast| format!("${ordinal}::{cast}")));
    }
    let value = parameter
        .value()
        .ok_or_else(|| RenderError::ValuelessParameter {
            name: name.to_owned(),
        })?;
    let Some(bound) = bind_of(value, name)? else {
        return Ok(None);
    };
    binds.push(bound);
    let ordinal = binds.len();
    slots.push((name.to_owned(), ordinal));
    let cast = cast_of(parameter, name)?.unwrap_or("text");
    Ok(Some(format!("${ordinal}::{cast}")))
}

/// The `PostgreSQL` type a parameter's value and hint imply.
///
/// `Ok(None)` is SQL `NULL`, which carries no type at all.
fn cast_of(parameter: &SqlParameter, name: &str) -> Result<Option<&'static str>, RenderError> {
    let value = parameter
        .value()
        .ok_or_else(|| RenderError::ValuelessParameter {
            name: name.to_owned(),
        })?;
    Ok(match value {
        Field::IsNull(true) => None,
        Field::BooleanValue(_) => Some("boolean"),
        Field::LongValue(_) => Some("bigint"),
        Field::BlobValue(_) => Some("bytea"),
        Field::StringValue(_) => Some(match parameter.type_hint() {
            Some(TypeHint::Uuid) => "uuid",
            Some(TypeHint::Json) => "jsonb",
            Some(TypeHint::Decimal) => "numeric",
            _ => "text",
        }),
        Field::DoubleValue(_) => {
            return Err(RenderError::DoubleParameter {
                name: name.to_owned(),
            });
        }
        _ => {
            return Err(RenderError::UnbindableParameter {
                name: name.to_owned(),
            });
        }
    })
}

/// The value to bind for a parameter, or `None` for SQL `NULL`.
fn bind_of(value: &Field, name: &str) -> Result<Option<Bound>, RenderError> {
    Ok(match value {
        Field::IsNull(true) => None,
        Field::BooleanValue(value) => Some(Bound::Bool(*value)),
        Field::LongValue(value) => Some(Bound::I64(*value)),
        Field::StringValue(value) => Some(Bound::Text(value.clone())),
        Field::BlobValue(value) => Some(Bound::Bytes(value.as_ref().to_vec())),
        Field::DoubleValue(_) => {
            return Err(RenderError::DoubleParameter {
                name: name.to_owned(),
            });
        }
        _ => {
            return Err(RenderError::UnbindableParameter {
                name: name.to_owned(),
            });
        }
    })
}
