//! Reading hyperfine's `--export-json` output. hyperfine 2 keeps one record
//! per run under `measurements`; hyperfine 1 keeps a `times` array. A
//! baseline may come from either.

use serde_json::Value;

/// One command's runs.
pub struct Case {
    pub name: String,
    /// Wall-clock seconds, one per run.
    pub times: Vec<f64>,
    /// Mean peak resident memory in bytes, when hyperfine measured it.
    pub peak_rss: Option<f64>,
    /// Mean instructions retired, when the platform gave hyperfine the
    /// counter (Linux and macOS with hyperfine 2).
    pub instructions: Option<f64>,
}

pub fn parse(text: &str) -> Result<Vec<Case>, String> {
    let json: Value = serde_json::from_str(text).map_err(|e| format!("hyperfine JSON: {e}"))?;
    let results = json["results"].as_array().ok_or("hyperfine JSON has no results")?;
    results.iter().map(parse_case).collect()
}

fn parse_case(result: &Value) -> Result<Case, String> {
    let name = result["name"]
        .as_str()
        .or_else(|| result["command"].as_str())
        .ok_or("a hyperfine result has no name")?
        .to_string();
    if let Some(runs) = result["measurements"].as_array() {
        let per_run = |key: &str| -> Vec<f64> {
            runs.iter().filter_map(|run| run[key]["value"].as_f64()).collect()
        };
        let times = per_run("time_wall_clock");
        if times.is_empty() {
            return Err(format!("{name}: no wall-clock times"));
        }
        return Ok(Case {
            name,
            times,
            peak_rss: mean_of(&per_run("memory_peak_resident")),
            instructions: mean_of(&per_run("instructions")),
        });
    }
    let numbers = |key: &str| -> Vec<f64> {
        result[key]
            .as_array()
            .map_or(Vec::new(), |xs| xs.iter().filter_map(Value::as_f64).collect())
    };
    let times = numbers("times");
    if times.is_empty() {
        return Err(format!("{name}: no times"));
    }
    Ok(Case { name, times, peak_rss: mean_of(&numbers("memory_usage_byte")), instructions: None })
}

fn mean_of(xs: &[f64]) -> Option<f64> {
    (!xs.is_empty()).then(|| super::stats::mean(xs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_hyperfine_2_measurements() {
        let text = r#"{"results": [{"command": "./x", "name": "nqueens/rust",
            "measurements": [
              {"time_wall_clock": {"value": 1.0, "unit": "second"},
               "memory_peak_resident": {"value": 100.0, "unit": "byte"},
               "instructions": {"value": 10.0}},
              {"time_wall_clock": {"value": 3.0, "unit": "second"},
               "memory_peak_resident": {"value": 300.0, "unit": "byte"},
               "instructions": {"value": 30.0}}]}]}"#;
        let cases = parse(text).unwrap();
        assert_eq!(cases[0].name, "nqueens/rust");
        assert_eq!(cases[0].times, vec![1.0, 3.0]);
        assert_eq!(cases[0].peak_rss, Some(200.0));
        assert_eq!(cases[0].instructions, Some(20.0));
    }

    #[test]
    fn reads_hyperfine_1_times() {
        let text = r#"{"results": [{"command": "qsort/flux", "mean": 2.0,
            "times": [1.5, 2.5], "memory_usage_byte": [10, 30]}]}"#;
        let cases = parse(text).unwrap();
        assert_eq!(cases[0].name, "qsort/flux");
        assert_eq!(cases[0].times, vec![1.5, 2.5]);
        assert_eq!(cases[0].peak_rss, Some(20.0));
        assert_eq!(cases[0].instructions, None);
    }
}
