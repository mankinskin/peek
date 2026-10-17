use crate::{
    error::CompactTerminalError,
    request::ReadSpillRequest,
    response::ReadSpillResult,
};

/// Read from a spill file with optional line range or grep.
pub fn read_spill(
    request: &ReadSpillRequest
) -> Result<ReadSpillResult, CompactTerminalError> {
    let content =
        std::fs::read_to_string(&request.spill_file).map_err(|source| {
            CompactTerminalError::CannotReadSpillFile {
                path: request.spill_file.clone(),
                source,
            }
        })?;

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    // grep mode: return matching line numbers.
    if let Some(ref pattern) = request.grep {
        let matches: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains(pattern.as_str()))
            .map(|(i, _)| i + 1)
            .collect();

        let text = if matches.is_empty() {
            format!("no match for {:?} in {} lines", pattern, total)
        } else {
            format!(
                "matches (line numbers): {}\ntotal: {} of {} lines matched",
                matches
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                matches.len(),
                total
            )
        };
        return Ok(ReadSpillResult { content: text });
    }

    // Bounded window mode.
    let start = request.start.unwrap_or(1).max(1);
    let end = request
        .end
        .unwrap_or_else(|| (start + 80).min(total))
        .min(total);

    if start > total {
        return Err(CompactTerminalError::InvalidRequest(format!(
            "start={start} exceeds spill file length ({total} lines)"
        )));
    }

    let window: String = lines[start - 1..=end.min(total) - 1]
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{:>6} {l}", start + i))
        .collect::<Vec<_>>()
        .join("\n");

    let header = format!(
        "# spill: {}, lines {start}–{end} of {total}\n",
        request.spill_file.display()
    );
    Ok(ReadSpillResult {
        content: format!("{header}{window}"),
    })
}
