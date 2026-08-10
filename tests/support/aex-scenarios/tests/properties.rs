//! What the Data `API` translation must hold for every committed statement.
//!
//! This target runs on the ordinary unit lane: it links no driver, starts no
//! container and reaches no network. It exists because the two halves of the
//! translation are pure functions over text, and a pure function that only ever
//! runs behind a container is a pure function nobody has tested.
//!
//! The corpus is not a sample. It is **every** `pub const … : &str` in the two
//! central statement modules, read from the source at compile time, so a
//! statement added tomorrow is covered tomorrow rather than when somebody
//! remembers to extend a list. That is the property that matters: a repository
//! author must not be able to write a statement the scenario transport silently
//! mangles.

use aex_scenarios::data_api::literal::{
    Projection, field, parse_array_literal, parse_row_literal, projection_of,
};
use aex_scenarios::data_api::render::{Bound, RenderError, Shape, render};
use aws_sdk_rdsdata::types::{ArrayValue, Field, SqlParameter, TypeHint};
use aws_smithy_types::Blob;

/// The committed control statements, as source.
const CONTROL_SQL: &str = include_str!("../../../../crates/aex-control-aurora/src/sql.rs");
/// The committed identity statements, as source.
const IDENTITY_SQL: &str = include_str!("../../../../crates/aex-identity-aurora/src/sql.rs");

// ---------------------------------------------------------------------------
// the corpus
// ---------------------------------------------------------------------------

/// Every `pub const NAME: &str = "…";` in one module's source.
///
/// Written out rather than pulled in as a dependency because the constants have
/// no runtime enumeration: a `pub use` would still need each name typed once,
/// which is exactly the list that goes stale.
fn statements(source: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("pub const ") {
        rest = &rest[at + "pub const ".len()..];
        let Some(colon) = rest.find(':') else { break };
        let name = rest[..colon].trim().to_owned();
        let Some(equals) = rest.find('=') else { break };
        let after = rest[equals + 1..].trim_start();
        if !after.starts_with('"') {
            rest = &rest[equals..];
            continue;
        }
        let (value, consumed) = rust_string(&after[1..]);
        found.push((name, value));
        let offset = after.as_ptr() as usize - rest.as_ptr() as usize;
        rest = &rest[offset + 1 + consumed..];
    }
    found
}

/// Unescapes one Rust string literal body, returning it and the bytes consumed.
///
/// Handles exactly the escapes these modules use, including the line
/// continuation `\` + newline that lets a statement be laid out over many lines
/// and still be one `&str`.
fn rust_string(body: &str) -> (String, usize) {
    let bytes = body.as_bytes();
    let mut out = String::new();
    let mut index = 0_usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return (out, index + 1),
            b'\\' => {
                let next = bytes.get(index + 1).copied().unwrap_or(b'"');
                index += 2;
                match next {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'0' => out.push('\0'),
                    b'\n' => {
                        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
                            index += 1;
                        }
                    }
                    other => out.push(char::from(other)),
                }
            }
            _ => {
                let character = body[index..].chars().next().unwrap_or('\0');
                out.push(character);
                index += character.len_utf8();
            }
        }
    }
    (out, index)
}

/// The `:name` parameters a statement binds, in first-appearance order.
fn parameter_names(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut names = Vec::new();
    let mut index = 0_usize;
    while index < bytes.len() {
        if bytes[index] == b':' && bytes.get(index + 1) == Some(&b':') {
            index += 2;
            continue;
        }
        if bytes[index] == b':'
            && bytes
                .get(index + 1)
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            let mut cursor = index + 1;
            while bytes
                .get(cursor)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                cursor += 1;
            }
            let name = sql[index + 1..cursor].to_owned();
            if !names.contains(&name) {
                names.push(name);
            }
            index = cursor;
            continue;
        }
        index += 1;
    }
    names
}

/// A text parameter, which is what the corpus check binds for every name.
fn text(name: &str, value: &str) -> SqlParameter {
    SqlParameter::builder()
        .name(name)
        .value(Field::StringValue(value.to_owned()))
        .build()
}

// ---------------------------------------------------------------------------
// the corpus properties
// ---------------------------------------------------------------------------

#[test]
fn the_corpus_is_not_empty_and_covers_both_statement_modules() {
    // A source scan that silently matched nothing would make every property
    // below vacuous, which is the one failure mode a corpus test has.
    assert!(
        statements(CONTROL_SQL).len() >= 50,
        "the control statement scan found {} statements",
        statements(CONTROL_SQL).len()
    );
    assert!(
        statements(IDENTITY_SQL).len() >= 25,
        "the identity statement scan found {} statements",
        statements(IDENTITY_SQL).len()
    );
}

#[test]
fn every_committed_statement_renders() {
    for (module, source) in [("control", CONTROL_SQL), ("identity", IDENTITY_SQL)] {
        for (name, sql) in statements(source) {
            let names = parameter_names(&sql);
            let parameters: Vec<SqlParameter> = names
                .iter()
                .map(|parameter| text(parameter, "00000000-0000-0000-0000-000000000000"))
                .collect();
            let rendered = render(&sql, &parameters)
                .unwrap_or_else(|error| panic!("{module}::{name} does not render: {error}"));
            assert_eq!(
                rendered.binds.len(),
                names.len(),
                "{module}::{name} bound {} value(s) for {} parameter(s)",
                rendered.binds.len(),
                names.len()
            );
        }
    }
}

#[test]
fn no_rendered_statement_keeps_a_named_placeholder() {
    for (module, source) in [("control", CONTROL_SQL), ("identity", IDENTITY_SQL)] {
        for (name, sql) in statements(source) {
            let parameters: Vec<SqlParameter> = parameter_names(&sql)
                .iter()
                .map(|parameter| text(parameter, "x"))
                .collect();
            let rendered = render(&sql, &parameters).expect("the statement renders");
            assert!(
                parameter_names(&rendered.sql).is_empty(),
                "{module}::{name} still binds {:?} after rendering",
                parameter_names(&rendered.sql)
            );
        }
    }
}

#[test]
fn every_committed_statement_that_projects_rows_is_wrapped_to_be_readable() {
    // A statement whose rows are never read back is a statement whose result
    // this transport would drop on the floor. The wrap is what makes a record
    // arrive at all, so "projects rows" and "is wrapped" must be the same set.
    for (module, source) in [("control", CONTROL_SQL), ("identity", IDENTITY_SQL)] {
        for (name, sql) in statements(source) {
            let parameters: Vec<SqlParameter> = parameter_names(&sql)
                .iter()
                .map(|parameter| text(parameter, "x"))
                .collect();
            let rendered = render(&sql, &parameters).expect("the statement renders");
            let upper = sql.to_ascii_uppercase();
            let projects =
                upper.trim_start().starts_with("SELECT") || upper.contains(" RETURNING ");
            assert_eq!(
                rendered.returns_rows(),
                projects,
                "{module}::{name} projects rows = {projects} but renders as {:?}",
                rendered.shape
            );
        }
    }
}

// ---------------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------------

#[test]
fn a_cast_operator_is_not_read_as_a_parameter() {
    let rendered = render(
        "SELECT (EXTRACT(EPOCH FROM created_at)*1000)::bigint FROM control.workspace WHERE id = :id",
        &[SqlParameter::builder()
            .name("id")
            .value(Field::StringValue("7f3a".to_owned()))
            .type_hint(TypeHint::Uuid)
            .build()],
    )
    .expect("renders");
    assert!(rendered.sql.contains("::bigint"), "{}", rendered.sql);
    assert!(rendered.sql.contains("$1::uuid"), "{}", rendered.sql);
    assert_eq!(rendered.binds, vec![Bound::Text("7f3a".to_owned())]);
}

#[test]
fn a_colon_inside_a_string_literal_is_left_alone() {
    let rendered = render(
        "SELECT 'scope:read' AS s, :actual FROM control.api_key",
        &[text("actual", "v")],
    )
    .expect("renders");
    assert!(rendered.sql.contains("'scope:read'"), "{}", rendered.sql);
    assert_eq!(rendered.binds.len(), 1);
}

#[test]
fn a_colon_inside_a_quoted_identifier_is_left_alone() {
    let rendered = render(
        "SELECT \"odd:column\" FROM control.workspace WHERE id = :id",
        &[text("id", "v")],
    )
    .expect("renders");
    assert!(rendered.sql.contains("\"odd:column\""), "{}", rendered.sql);
    assert_eq!(rendered.binds.len(), 1);
}

#[test]
fn a_repeated_parameter_binds_once_and_reuses_its_slot() {
    let rendered = render(
        "SELECT :now_ms, :other, :now_ms FROM control.workspace",
        &[text("now_ms", "1"), text("other", "2")],
    )
    .expect("renders");
    assert_eq!(rendered.binds.len(), 2);
    assert_eq!(
        rendered.sql.matches("$1::text").count(),
        2,
        "{}",
        rendered.sql
    );
    assert_eq!(
        rendered.sql.matches("$2::text").count(),
        1,
        "{}",
        rendered.sql
    );
}

#[test]
fn every_type_hint_becomes_the_cast_the_service_would_have_applied() {
    let table = [
        (Some(TypeHint::Uuid), "$1::uuid"),
        (Some(TypeHint::Json), "$1::jsonb"),
        (Some(TypeHint::Decimal), "$1::numeric"),
        (None, "$1::text"),
    ];
    for (hint, expected) in table {
        let mut builder = SqlParameter::builder()
            .name("value")
            .value(Field::StringValue("v".to_owned()));
        if let Some(hint) = hint.clone() {
            builder = builder.type_hint(hint);
        }
        let rendered =
            render("SELECT :value FROM control.workspace", &[builder.build()]).expect("renders");
        assert!(
            rendered.sql.contains(expected),
            "{hint:?} rendered as {}",
            rendered.sql
        );
    }
}

#[test]
fn a_boolean_a_bigint_and_a_bytea_carry_their_own_casts() {
    let rendered = render(
        "SELECT :flag, :count, :blob FROM control.workspace",
        &[
            SqlParameter::builder()
                .name("flag")
                .value(Field::BooleanValue(true))
                .build(),
            SqlParameter::builder()
                .name("count")
                .value(Field::LongValue(7))
                .build(),
            SqlParameter::builder()
                .name("blob")
                .value(Field::BlobValue(Blob::new(vec![1_u8, 2])))
                .build(),
        ],
    )
    .expect("renders");
    assert!(rendered.sql.contains("$1::boolean"), "{}", rendered.sql);
    assert!(rendered.sql.contains("$2::bigint"), "{}", rendered.sql);
    assert!(rendered.sql.contains("$3::bytea"), "{}", rendered.sql);
    assert_eq!(
        rendered.binds,
        vec![
            Bound::Bool(true),
            Bound::I64(7),
            Bound::Bytes(vec![1_u8, 2])
        ]
    );
}

#[test]
fn a_null_parameter_is_inlined_as_the_untyped_keyword() {
    // A Data API null carries no type and coerces from context. A bound `text`
    // null does not, and would fail against every non-text column, so the null
    // must not become a bind.
    let rendered = render(
        "INSERT INTO identity.user (name) VALUES (:name)",
        &[SqlParameter::builder()
            .name("name")
            .value(Field::IsNull(true))
            .build()],
    )
    .expect("renders");
    assert!(rendered.sql.contains("VALUES (NULL)"), "{}", rendered.sql);
    assert!(rendered.binds.is_empty());
}

#[test]
fn a_double_parameter_is_refused_at_the_boundary() {
    let error = render(
        "SELECT :ratio",
        &[SqlParameter::builder()
            .name("ratio")
            .value(Field::DoubleValue(1.5))
            .build()],
    )
    .expect_err("a double never crosses this boundary");
    assert_eq!(
        error,
        RenderError::DoubleParameter {
            name: "ratio".to_owned()
        }
    );
}

#[test]
fn a_statement_binding_a_name_nobody_supplies_is_refused() {
    assert_eq!(
        render("SELECT :missing", &[]).expect_err("no parameter supplies it"),
        RenderError::MissingParameter {
            name: "missing".to_owned()
        }
    );
}

#[test]
fn a_supplied_parameter_the_statement_ignores_is_tolerated() {
    // Not laxity: the Data API ignores a parameter the statement does not
    // reference, so refusing one here would make this transport stricter than
    // the service the statements were written for. The case that motivated it
    // was `DENY_DEVICE_AUTHORIZATION` referencing two of the four parameters
    // `decide_device` binds, which was a defect and is now fixed; the tolerance
    // outlives it because it describes the service, not that statement.
    let rendered = render("SELECT 1", &[text("spare", "v")]).expect("renders");
    assert!(rendered.binds.is_empty());
}

#[test]
fn each_statement_shape_gets_the_wrap_postgres_admits() {
    let select = render("SELECT id FROM control.workspace", &[]).expect("renders");
    assert_eq!(select.shape, Shape::Subquery);
    assert!(!select.dml);

    let returning = render(
        "INSERT INTO control.workspace (id) VALUES (gen_random_uuid()) RETURNING id",
        &[],
    )
    .expect("renders");
    assert_eq!(returning.shape, Shape::Cte);
    assert!(returning.dml);

    let blind = render("UPDATE control.workspace SET status = 'active'", &[]).expect("renders");
    assert_eq!(blind.shape, Shape::Plain);
    assert!(blind.dml);
    assert!(!blind.returns_rows());

    let isolation = render("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE", &[]).expect("renders");
    assert_eq!(isolation.shape, Shape::Plain);
    assert!(!isolation.dml);
}

#[test]
fn a_leading_with_is_merged_rather_than_nested() {
    // PostgreSQL refuses a data-modifying statement in a WITH clause that is not
    // at the top level, so wrapping `WITH … UPDATE … RETURNING` inside a second
    // WITH would fail outright. Merging into the existing list keeps it legal.
    let rendered = render(
        "WITH due AS (SELECT id FROM control.outbox_message LIMIT 10) \
         UPDATE control.outbox_message SET claimed_at = now() FROM due \
         WHERE control.outbox_message.id = due.id RETURNING control.outbox_message.id",
        &[],
    )
    .expect("renders");
    assert_eq!(rendered.shape, Shape::Merge);
    assert!(rendered.dml);
    assert!(
        rendered.sql.starts_with("WITH due AS ("),
        "{}",
        rendered.sql
    );
    assert_eq!(
        rendered.sql.matches("WITH").count(),
        1,
        "the merge must not introduce a second WITH: {}",
        rendered.sql
    );
    assert!(rendered.sql.contains("::text AS"), "{}", rendered.sql);
}

#[test]
fn a_select_for_update_is_a_read_and_never_counts_rows_affected() {
    let rendered = render(
        "SELECT id FROM control.invitation ORDER BY id LIMIT 100 FOR UPDATE",
        &[],
    )
    .expect("renders");
    assert_eq!(rendered.shape, Shape::Subquery);
    assert!(
        !rendered.dml,
        "`FOR UPDATE` is a locking clause, not a write"
    );
}

#[test]
fn a_dollar_sign_outside_a_literal_is_refused_rather_than_guessed_at() {
    assert_eq!(
        render("SELECT $$body$$", &[]).expect_err("dollar quoting is not admitted"),
        RenderError::ReservedDollar
    );
}

// ---------------------------------------------------------------------------
// reading rows back
// ---------------------------------------------------------------------------

#[test]
fn a_composite_literal_distinguishes_null_from_the_empty_string() {
    assert_eq!(
        parse_row_literal("(a,,\"\")").expect("parses"),
        vec![Some("a".to_owned()), None, Some(String::new())]
    );
    assert_eq!(parse_row_literal("()").expect("parses"), vec![None]);
}

#[test]
fn a_composite_literal_keeps_a_comma_inside_its_quotes() {
    assert_eq!(
        parse_row_literal("(\"a,b\",c)").expect("parses"),
        vec![Some("a,b".to_owned()), Some("c".to_owned())]
    );
}

#[test]
fn a_composite_literal_unescapes_quotes_and_backslashes() {
    assert_eq!(
        parse_row_literal("(\"say \"\"hi\"\"\",\"\\\\x0102\")").expect("parses"),
        vec![Some("say \"hi\"".to_owned()), Some("\\x0102".to_owned())]
    );
}

#[test]
fn a_row_that_is_not_a_composite_is_refused() {
    assert!(parse_row_literal("a,b").is_err());
}

#[test]
fn an_array_literal_reads_null_only_when_it_is_unquoted() {
    assert_eq!(
        parse_array_literal("{a,NULL,\"NULL\"}").expect("parses"),
        vec![Some("a".to_owned()), None, Some("NULL".to_owned())]
    );
    assert_eq!(
        parse_array_literal("{}").expect("parses"),
        Vec::<Option<String>>::new()
    );
}

#[test]
fn every_postgres_type_projects_the_variant_the_service_answers_with() {
    let table = [
        ("BOOL", Projection::Bool),
        ("INT2", Projection::Long),
        ("INT4", Projection::Long),
        ("INT8", Projection::Long),
        ("BYTEA", Projection::Blob),
        ("TEXT[]", Projection::TextArray),
        ("FLOAT8", Projection::Double),
        ("UUID", Projection::Text),
        ("NUMERIC", Projection::Text),
        ("JSONB", Projection::Text),
        ("TIMESTAMPTZ", Projection::Text),
        ("account_kind", Projection::Text),
    ];
    for (postgres_type, expected) in table {
        assert_eq!(projection_of(postgres_type), expected, "{postgres_type}");
    }
}

#[test]
fn a_null_column_becomes_is_null_whatever_its_type() {
    for postgres_type in ["BOOL", "INT8", "BYTEA", "TEXT[]", "NUMERIC"] {
        assert_eq!(
            field(0, postgres_type, None).expect("a null decodes"),
            Field::IsNull(true),
            "{postgres_type}"
        );
    }
}

#[test]
fn each_type_decodes_into_the_field_the_record_accessor_reads() {
    assert_eq!(
        field(0, "BOOL", Some("t")).expect("decodes"),
        Field::BooleanValue(true)
    );
    assert_eq!(
        field(0, "INT8", Some("-7")).expect("decodes"),
        Field::LongValue(-7)
    );
    assert_eq!(
        field(0, "BYTEA", Some("\\x0102")).expect("decodes"),
        Field::BlobValue(Blob::new(vec![1_u8, 2]))
    );
    assert_eq!(
        field(0, "TEXT[]", Some("{a,b}")).expect("decodes"),
        Field::ArrayValue(ArrayValue::StringValues(vec![
            Some("a".to_owned()),
            Some("b".to_owned())
        ]))
    );
    assert_eq!(
        field(0, "NUMERIC", Some("12345.678901")).expect("decodes"),
        Field::StringValue("12345.678901".to_owned()),
        "a NUMERIC keeps its exact decimal spelling, which is the whole reason \
         the row is read as text"
    );
}

#[test]
fn a_column_whose_text_contradicts_its_type_is_a_loud_failure() {
    assert!(field(0, "BOOL", Some("yes")).is_err());
    assert!(field(0, "INT8", Some("nine")).is_err());
    assert!(field(0, "BYTEA", Some("0102")).is_err());
}
