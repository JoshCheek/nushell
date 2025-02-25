use crate::prelude::*;
use indexmap::set::IndexSet;
use log::trace;
use nu_engine::WholeStreamCommand;
use nu_errors::ShellError;
use nu_protocol::{
    did_you_mean, ColumnPath, Dictionary, PathMember, Primitive, ReturnSuccess, Signature,
    SyntaxShape, UnspannedPathMember, UntaggedValue, Value,
};
use nu_source::HasFallibleSpan;
use nu_value_ext::get_data_by_column_path;

pub struct Command;

impl WholeStreamCommand for Command {
    fn name(&self) -> &str {
        "get"
    }

    fn signature(&self) -> Signature {
        Signature::build("get").rest(
            "rest",
            SyntaxShape::ColumnPath,
            "optionally return additional data by path",
        )
    }

    fn usage(&self) -> &str {
        "Open given cells as text."
    }

    fn run_with_actions(&self, args: CommandArgs) -> Result<ActionStream, ShellError> {
        get(args)
    }

    fn examples(&self) -> Vec<Example> {
        vec![
            Example {
                description: "Extract the name of files as a list",
                example: "ls | get name",
                result: None,
            },
            Example {
                description: "Extract the cpu list from the sys information",
                example: "sys | get cpu",
                result: None,
            },
        ]
    }
}

pub fn get(args: CommandArgs) -> Result<ActionStream, ShellError> {
    let column_paths: Vec<ColumnPath> = args.rest(0)?;
    let mut input = args.input;

    if column_paths.is_empty() {
        let vec = input.drain_vec();

        let descs = nu_protocol::merge_descriptors(&vec);

        Ok(descs
            .into_iter()
            .map(ReturnSuccess::value)
            .into_action_stream())
    } else {
        trace!("get {:?}", column_paths);
        let output_stream = input
            .flat_map(move |item| {
                column_paths
                    .iter()
                    .flat_map(move |path| get_output(&item, path))
                    .collect::<Vec<_>>()
            })
            .into_action_stream();
        Ok(output_stream)
    }
}

fn get_output(item: &Value, path: &ColumnPath) -> Vec<Result<ReturnSuccess, ShellError>> {
    match get_column_path(path, item) {
        Ok(Value {
            value: UntaggedValue::Primitive(Primitive::Nothing),
            ..
        }) => vec![],
        Ok(Value {
            value: UntaggedValue::Table(rows),
            ..
        }) => rows.into_iter().map(ReturnSuccess::value).collect(),
        Ok(other) => vec![ReturnSuccess::value(other)],
        Err(reason) => vec![ReturnSuccess::value(
            UntaggedValue::Error(reason).into_untagged_value(),
        )],
    }
}

pub fn get_column_path(path: &ColumnPath, obj: &Value) -> Result<Value, ShellError> {
    get_data_by_column_path(obj, path, move |obj_source, column_path_tried, error| {
        let path_members_span = path.maybe_span().unwrap_or_else(Span::unknown);

        match &obj_source.value {
            UntaggedValue::Table(rows) => {
                return get_column_path_from_table_error(
                    rows,
                    column_path_tried,
                    &path_members_span,
                );
            }
            UntaggedValue::Row(columns) => {
                if let Some(error) = get_column_from_row_error(
                    columns,
                    column_path_tried,
                    &path_members_span,
                    obj_source,
                ) {
                    return error;
                }
            }
            _ => {}
        }

        if let Some(suggestions) = did_you_mean(obj_source, column_path_tried.as_string()) {
            ShellError::labeled_error(
                "Unknown column",
                format!("did you mean '{}'?", suggestions[0]),
                column_path_tried.span.since(path_members_span),
            )
        } else {
            error
        }
    })
}

pub fn get_column_path_from_table_error(
    rows: &[Value],
    column_path_tried: &PathMember,
    path_members_span: &Span,
) -> ShellError {
    match column_path_tried {
        PathMember {
            unspanned: UnspannedPathMember::String(column),
            ..
        } => {
            let primary_label = format!("There isn't a column named '{}'", &column);

            let suggestions: IndexSet<_> = rows
                .iter()
                .filter_map(|r| did_you_mean(r, column_path_tried.as_string()))
                .map(|s| s[0].to_owned())
                .collect();
            let mut existing_columns: IndexSet<_> = IndexSet::default();
            let mut names: Vec<String> = vec![];

            for row in rows {
                for field in row.data_descriptors() {
                    if !existing_columns.contains(&field[..]) {
                        existing_columns.insert(field.clone());
                        names.push(field);
                    }
                }
            }

            if names.is_empty() {
                ShellError::labeled_error_with_secondary(
                    "Unknown column",
                    primary_label,
                    column_path_tried.span,
                    "Appears to contain rows. Try indexing instead.",
                    column_path_tried.span.since(path_members_span),
                )
            } else {
                ShellError::labeled_error_with_secondary(
                    "Unknown column",
                    primary_label,
                    column_path_tried.span,
                    format!(
                        "Perhaps you meant '{}'? Columns available: {}",
                        suggestions
                            .iter()
                            .map(|x| x.to_owned())
                            .collect::<Vec<String>>()
                            .join(","),
                        names.join(", ")
                    ),
                    column_path_tried.span.since(path_members_span),
                )
            }
        }
        PathMember {
            unspanned: UnspannedPathMember::Int(idx),
            ..
        } => {
            let total = rows.len();

            let secondary_label = if total == 1 {
                "The table only has 1 row".to_owned()
            } else {
                format!("The table only has {} rows (0 to {})", total, total - 1)
            };

            ShellError::labeled_error_with_secondary(
                "Row not found",
                format!("There isn't a row indexed at {}", idx),
                column_path_tried.span,
                secondary_label,
                column_path_tried.span.since(path_members_span),
            )
        }
    }
}

pub fn get_column_from_row_error(
    columns: &Dictionary,
    column_path_tried: &PathMember,
    path_members_span: &Span,
    obj_source: &Value,
) -> Option<ShellError> {
    match column_path_tried {
        PathMember {
            unspanned: UnspannedPathMember::String(column),
            ..
        } => {
            let primary_label = format!("There isn't a column named '{}'", &column);

            did_you_mean(obj_source, column_path_tried.as_string()).map(|suggestions| {
                ShellError::labeled_error_with_secondary(
                    "Unknown column",
                    primary_label,
                    column_path_tried.span,
                    format!(
                        "Perhaps you meant '{}'? Columns available: {}",
                        suggestions[0],
                        &obj_source.data_descriptors().join(", ")
                    ),
                    column_path_tried.span.since(path_members_span),
                )
            })
        }
        PathMember {
            unspanned: UnspannedPathMember::Int(idx),
            ..
        } => Some(ShellError::labeled_error_with_secondary(
            "No rows available",
            format!("A row at '{}' can't be indexed.", &idx),
            column_path_tried.span,
            format!(
                "Appears to contain columns. Columns available: {}",
                columns.keys().join(", ")
            ),
            column_path_tried.span.since(path_members_span),
        )),
    }
}
