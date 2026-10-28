//! Present native OpenCode forms through Del's existing question surface.
use crate::mapping::string;
use del_agent::{
    Result, SessionId,
    backend::{QuestionAnswer, QuestionHandler},
};
use serde_json::{Value, json};

fn visible(field: &Value, answers: &Value) -> bool {
    field["when"]
        .as_array()
        .into_iter()
        .flatten()
        .all(|condition| {
            let answer = &answers[string(condition, "key")];
            if answer.is_null() {
                return false;
            }
            let equal = answer == &condition["value"]
                || answer
                    .as_array()
                    .is_some_and(|a| a.contains(&condition["value"]));
            if condition["op"] == "neq" {
                !equal
            } else {
                equal
            }
        })
}
pub async fn answer(id: SessionId, form: &Value, ask: &QuestionHandler) -> Result<Option<Value>> {
    let mut answers = json!({});
    for field in form["fields"].as_array().into_iter().flatten() {
        if !visible(field, &answers) {
            continue;
        }
        let key = string(field, "key");
        if field["hidden"] == true {
            if !field["default"].is_null() {
                answers[&key] = field["default"].clone();
            }
            continue;
        }
        let title = field["title"].as_str().unwrap_or(&key);
        let prompt = format!(
            "{} — {}\n{}",
            string(form, "title"),
            title,
            string(field, "description")
        );
        let choices: Vec<_> = field["options"].as_array().into_iter().flatten().collect();
        let kind = field["type"].as_str().unwrap_or("string");
        if kind == "external" {
            if ask(
                id.clone(),
                format!("{prompt}\n{}", string(field, "url")),
                vec!["Continue".into()],
            )
            .await?
            .is_none()
            {
                return Ok(None);
            }
            continue;
        }
        let mut selected: Vec<String> = vec![];
        loop {
            let mut labels: Vec<String> = choices.iter().map(|c| string(c, "label")).collect();
            if kind == "boolean" {
                labels = vec!["Yes".into(), "No".into()];
            }
            if kind == "multiselect" {
                labels = choices
                    .iter()
                    .map(|c| {
                        format!(
                            "{}{}",
                            if selected.contains(&string(c, "value")) {
                                "✓ "
                            } else {
                                ""
                            },
                            string(c, "label")
                        )
                    })
                    .collect();
                labels.push("Done".into());
            }
            let Some(response) = ask(id.clone(), prompt.clone(), labels).await? else {
                return Ok(None);
            };
            let value = match response {
                QuestionAnswer::Option(index) if kind == "boolean" => Value::Bool(index == 0),
                QuestionAnswer::Option(index) if kind == "multiselect" => {
                    if let Some(choice) = choices.get(index) {
                        let value = string(choice, "value");
                        if selected.contains(&value) {
                            selected.retain(|v| v != &value);
                        } else {
                            selected.push(value);
                        }
                        continue;
                    }
                    // Without custom entries, no more than the offered options can be
                    // selected; a larger minItems would make the form impossible.
                    let mut min_items = field["minItems"].as_u64().unwrap_or(0) as usize;
                    if field["custom"] != true {
                        min_items = min_items.min(choices.len());
                    }
                    if selected.len() < min_items
                        || field["maxItems"]
                            .as_u64()
                            .is_some_and(|n| selected.len() > n as usize)
                    {
                        continue;
                    }
                    json!(selected)
                }
                QuestionAnswer::Option(index) => match choices.get(index) {
                    Some(choice) => choice["value"].clone(),
                    None => continue,
                },
                QuestionAnswer::Text(text) => match kind {
                    "number" | "integer" => {
                        let Ok(number) = text.parse::<f64>() else {
                            continue;
                        };
                        if !number.is_finite()
                            || (kind == "integer" && number.fract() != 0.)
                            || field["minimum"].as_f64().is_some_and(|n| number < n)
                            || field["maximum"].as_f64().is_some_and(|n| number > n)
                        {
                            continue;
                        }
                        json!(number)
                    }
                    "boolean" => match text.to_lowercase().as_str() {
                        "yes" | "true" => json!(true),
                        "no" | "false" => json!(false),
                        _ => continue,
                    },
                    "multiselect" => {
                        if field["custom"] == true {
                            selected.push(text);
                        }
                        continue;
                    }
                    _ => {
                        if !choices.is_empty()
                            && field["custom"] != true
                            && !choices.iter().any(|c| c["value"] == text)
                        {
                            continue;
                        }
                        if field["required"] == true && text.is_empty() {
                            continue;
                        }
                        Value::String(text)
                    }
                },
            };
            answers[&key] = value;
            break;
        }
    }
    Ok(Some(answers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    fn scripted(responses: Vec<Option<QuestionAnswer>>) -> QuestionHandler {
        let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
        Arc::new(move |_, _, _| {
            let answer = responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected question");
            Box::pin(async move { Ok(answer) })
        })
    }

    #[tokio::test]
    async fn forms_preserve_types_and_conditional_fields() {
        let form = json!({"title":"Configure","fields":[
            {"key":"hidden","type":"string","hidden":true,"default":"native"},
            {"key":"enabled","type":"boolean"},
            {"key":"count","type":"integer","minimum":1,"maximum":5,
             "when":[{"key":"enabled","op":"eq","value":true}]},
            {"key":"skipped","type":"string","when":[{"key":"enabled","op":"eq","value":false}]},
            {"key":"items","type":"multiselect","minItems":1,
             "options":[{"label":"A","value":"a"},{"label":"B","value":"b"}]}
        ]});
        let ask = scripted(vec![
            Some(QuestionAnswer::Option(0)),
            Some(QuestionAnswer::Text("1.5".into())),
            Some(QuestionAnswer::Text("3".into())),
            Some(QuestionAnswer::Option(2)),
            Some(QuestionAnswer::Option(1)),
            Some(QuestionAnswer::Option(2)),
        ]);
        let answer = answer(SessionId::new("ses_test"), &form, &ask)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            answer,
            json!({"hidden":"native","enabled":true,"count":3.0,"items":["b"]})
        );
    }

    #[tokio::test]
    async fn multiselect_min_items_is_capped_by_the_offered_options() {
        let form = json!({"title":"Pick","fields":[
            {"key":"items","type":"multiselect","minItems":5,
             "options":[{"label":"A","value":"a"},{"label":"B","value":"b"}]}
        ]});
        // Two options can never satisfy minItems 5; the cap (2) lets the form
        // complete once both are selected instead of asking forever.
        let ask = scripted(vec![
            Some(QuestionAnswer::Option(0)),
            Some(QuestionAnswer::Option(2)),
            Some(QuestionAnswer::Option(1)),
            Some(QuestionAnswer::Option(2)),
        ]);
        let answer = answer(SessionId::new("ses_test"), &form, &ask)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(answer, json!({"items":["a","b"]}));
    }

    #[tokio::test]
    async fn dismissing_a_question_cancels_the_whole_form() {
        let form = json!({"fields":[{"key":"answer","type":"string"}]});
        assert!(
            answer(SessionId::new("ses_test"), &form, &scripted(vec![None]))
                .await
                .unwrap()
                .is_none()
        );
    }
}
