use atvprep_api::opaque::Opaque;
use serde_json::Value;

pub fn value(value: &mut Value, key: &str, opaque: &Opaque) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                self::value(value, key, opaque);
            }
        }
        Value::Array(items) => {
            for value in items {
                self::value(value, "", opaque);
            }
        }
        Value::String(text) if key.ends_with("_at") => "<ts>".clone_into(text),
        Value::String(text) if key == "csrf_token" => "<csrf>".clone_into(text),
        Value::String(text) => {
            if let Some((kind, _)) = text.split_once('_')
                && let Ok(id) = opaque.decode(text, kind)
            {
                *text = format!("<{kind}:{id}>");
            }
        }
        _ => {}
    }
}

pub fn render(text: &str, opaque: &Opaque) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('>') else { break };
        let candidate = &rest[1..end];
        if let Some((kind, id)) = candidate.rsplit_once(':')
            && let Ok(id) = id.parse::<i64>()
        {
            result.push_str(&opaque.encode(kind, id));
        } else {
            result.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    result
}

pub fn cookie(value: &str) -> String {
    value.split_once(';').map_or_else(
        || "<cookie>".to_owned(),
        |(_, attributes)| format!("<cookie>;{attributes}"),
    )
}

pub fn cookie_view(value: &str) -> Value {
    let mut parts = value.split(';');
    let (name, credential) = parts
        .next()
        .unwrap_or("")
        .split_once('=')
        .unwrap_or(("", ""));
    let attributes: Vec<_> = parts
        .map(|value| value.trim().to_ascii_lowercase())
        .collect();
    if credential.trim_matches('"').is_empty() {
        return serde_json::json!({"name":name,"value":"","max_age":if attributes.iter().any(|value|value=="max-age=0") {Some(0)} else {None}});
    }
    let attribute = |name: &str| {
        attributes
            .iter()
            .find_map(|value| value.strip_prefix(name))
            .unwrap_or("")
    };
    serde_json::json!({"name":name,"value":"<cookie>","secure":attributes.iter().any(|value|value=="secure"),"http_only":attributes.iter().any(|value|value=="httponly"),"same_site":attribute("samesite="),"path":attribute("path=")})
}

pub fn response(value: &mut Value, actual_path: &str, recorded_path: &str, opaque: &Opaque) {
    if value.get("instance").is_some() {
        assert_eq!(
            value["instance"],
            actual_path.split('?').next().unwrap(),
            "actual error path"
        );
        value["instance"] = serde_json::json!(recorded_path.split('?').next().unwrap());
    }
    self::value(value, "", opaque);
}
