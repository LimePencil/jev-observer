//! Reproducible synthetic observations for the demo dashboard.

use super::normalize::normalize;
use super::types::{Capture, NormalizeOptions};
use serde_json::Value;
use serde_json::json;

/// A reproducible synthetic workload, not evidence of latency or model quality.
pub fn sample_records() -> Vec<Value> {
    let now = chrono::Utc::now().timestamp_millis();
    let options = NormalizeOptions {
        input_price_per_million: Some(0.042),
        output_price_per_million: Some(0.0),
        ..NormalizeOptions::default()
    };
    (0..720).map(|i| {
        let version = if i < 470 { 1 } else { 2 };
        let indexed = i % 9 == 0;
        let source = if indexed { "Code search" } else if i % 4 == 0 { "Commit checks" } else { "Support inbox" };
        let department = if i % 10 < 5 { "billing" } else if i % 10 < 8 { "technical" } else { "sales" };
        let mut probabilities = json!({"billing":0.1,"technical":0.1,"sales":0.1});
        probabilities[department] = json!(0.8);
        let urgency = ((i * 17 % 94) as f64 + 3.0) / 100.0;
        let level = (i % 3) as usize;
        let mut score_probabilities = json!({"0":0.1,"1":0.1,"2":0.1});
        score_probabilities[level.to_string()] = json!(0.8);
        let score: f64 = (0..3).map(|n| n as f64 * score_probabilities[n.to_string()].as_f64().unwrap()).sum();
        let mut questions = json!({
            "support_routing": {"type":"choice","instructions": if version == 1 { "Which team should handle this ticket?" } else { "Which team should handle this ticket? Route outages to technical even when payment is mentioned." },"criteria":{"billing":"Payments and refunds","technical":"Bugs and outages","sales":"Plans and upgrades"}},
            "urgency": {"type":"noul","instructions":"Does the customer need a response today?"},
            "frustration": {"type":"score","instructions":"How frustrated is the customer?","criteria":["Calm","Frustrated","Very angry"]}
        });
        let mut answers = json!({
            "support_routing":{"type":"choice","choice":department,"probabilities":probabilities,"confidence":0.72},
            "urgency":{"type":"noul","noul":urgency},
            "frustration":{"type":"score","score":score,"probabilities":score_probabilities,"legend":{"0":"Calm","1":"Frustrated","2":"Very angry"},"confidence":0.72}
        });
        if source == "Commit checks" {
            questions = json!({"breaking_change":{"type":"noul","instructions":"Does this commit change a public API?"},"review_priority":{"type":"score","instructions":"How much review does this change need?","criteria":["Routine","Careful review","Specialist review"]}});
            answers = json!({"breaking_change":{"type":"noul","noul":urgency},"review_priority":{"type":"score","score":score,"probabilities":score_probabilities,"legend":{"0":"Routine","1":"Careful review","2":"Specialist review"},"confidence":0.72}});
        }
        if indexed {
            questions = json!({}); answers = json!({});
            for n in 0..4 {
                let key = format!("c{n}");
                questions[&key] = json!({"type":"noul","instructions":format!("Look only at the chunk with id \"{key}\". Does that code match this description: handles retries with a limit of {}", if version == 1 { 3 } else { 5 })});
                answers[&key] = json!({"type":"noul","noul":((i + n * 13) % 100) as f64 / 100.0});
            }
        }
        let failed = i % 31 == 0;
        let response = if failed { json!({"error":{"message":"Synthetic upstream overload","type":"overloaded"}}) } else {
            json!({"model":if i % 6 == 0 { "jev-synthetic-b" } else { "jev-synthetic-a" },"answers":answers,"usage":{"input_tokens":240 + i % 1700,"output_tokens":24 + i % 40}})
        };
        let capture = Capture {
            id: format!("synthetic-{i:04}").into(), timestamp: now - (720 - i) as i64 * 110_000,
            source: source.into(), adapter: indexed.then(|| "jgrep-v1".into()),
            status: if failed { 529 } else { 200 },
            duration_ms: (45 + i * 37 % 380 + if i % 43 == 0 { 430 } else { 0 }) as f64,
            request: serde_json::to_vec(&json!({"model":"jev-synthetic","state":format!("Synthetic example {i}"),"questions":questions})).unwrap(),
            response: serde_json::to_vec(&response).unwrap(), capture_complete: true, ..Capture::default()
        };
        let mut record = normalize(&capture, &options);
        record["sample"] = json!(true);
        if !record["cost_usd"].is_null() { record["cost_basis"] = json!("synthetic"); }
        if !failed && !indexed && i % 13 == 0 {
            let key = record["answers"][0]["key"].clone();
            record["labels"] = json!([{"key":key,"label":if i % 39 == 0 {"incorrect"} else {"correct"},"source":"synthetic_fixture","timestamp":capture.timestamp}]);
        }
        record
    }).collect()
}
