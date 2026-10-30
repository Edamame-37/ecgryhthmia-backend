use crate::api::error::{ApiError, ApiResult};
use std::io::Read;

fn channel(header: &str) -> Option<usize> {
    let normalized = header
        .trim()
        .to_ascii_lowercase()
        .replace("(mV)", "")
        .replace("(mv)", "")
        .replace([' ', '_', '-'], "");
    match normalized.as_str() {
        "i" | "leadi" | "ch1" => Some(0),
        "ii" | "leadii" | "ch2" => Some(1),
        "iii" | "leadiii" | "ch3" => Some(2),
        _ => None,
    }
}
pub struct ParsedCsv {
    pub samples: Vec<Vec<f64>>,
    pub sampling_rate_hz: Option<f64>,
}
pub fn parse_csv_samples(reader: impl Read) -> ApiResult<Vec<Vec<f64>>> {
    Ok(parse_csv_frame(reader)?.samples)
}
pub fn parse_csv_frame(reader: impl Read) -> ApiResult<ParsedCsv> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(reader);
    let headers = reader
        .headers()
        .map_err(|_| ApiError::bad("CSV header missing"))?
        .clone();
    let mut columns = [None; 3];
    let mut time_column = None;
    for (index, header) in headers.iter().enumerate() {
        if let Some(channel) = channel(header) {
            if columns[channel].replace(index).is_some() {
                return Err(ApiError::bad("Duplicate ECG channel"));
            }
        } else if !matches!(
            header.trim().to_ascii_lowercase().as_str(),
            "time" | "timestamp" | "t" | "time(s)" | "time (s)" | "timestamp (s)"
        ) {
            return Err(ApiError::bad(format!("Unknown CSV column: {header}")));
        } else if time_column.replace(index).is_some() {
            return Err(ApiError::bad("Duplicate CSV time column"));
        }
    }
    if columns.iter().any(Option::is_none) {
        return Err(ApiError::bad("CSV requires Lead I, II and III"));
    }
    let mut samples = Vec::new();
    let mut previous_time: Option<f64> = None;
    let mut sample_interval: Option<f64> = None;
    for (index, row) in reader.records().enumerate() {
        let row = row.map_err(|_| ApiError::bad(format!("Invalid CSV row {}", index + 2)))?;
        if let Some(column) = time_column {
            let time = row
                .get(column)
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| ApiError::bad(format!("Invalid CSV time at row {}", index + 2)))?;
            if previous_time.is_some_and(|previous| time <= previous) {
                return Err(ApiError::bad("CSV time must increase strictly"));
            }
            if let Some(previous) = previous_time {
                let interval = time - previous;
                if let Some(expected) = sample_interval {
                    if (interval - expected).abs() > 1e-6f64.max(expected * 0.005) {
                        return Err(ApiError::bad("CSV sample timing is not uniform"));
                    }
                } else {
                    sample_interval = Some(interval);
                }
            }
            previous_time = Some(time);
        }
        let mut sample = vec![0.0; 3];
        for channel in 0..3 {
            let value = row
                .get(columns[channel].unwrap())
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && v.abs() <= 100.0)
                .ok_or_else(|| {
                    ApiError::bad(format!(
                        "Invalid ECG value at row {}, channel {}",
                        index + 2,
                        channel + 1
                    ))
                })?;
            sample[channel] = value;
        }
        samples.push(sample);
        if samples.len() > 30000 {
            return Err(ApiError::bad("CSV frame exceeds sample limit"));
        }
    }
    if samples.is_empty() {
        return Err(ApiError::bad("CSV contains no samples"));
    }
    let sampling_rate_hz = sample_interval.map(|interval| 1.0 / interval);
    if sampling_rate_hz.is_some_and(|rate| !(99.99..=1000.01).contains(&rate)) {
        return Err(ApiError::bad("Unsupported CSV sampling rate"));
    }
    Ok(ParsedCsv {
        samples,
        sampling_rate_hz,
    })
}
