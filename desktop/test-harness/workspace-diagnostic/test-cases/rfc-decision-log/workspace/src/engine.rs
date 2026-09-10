pub fn route_signal(topic: &str) -> &'static str {
    if topic.starts_with("critical") {
        "urgent"
    } else {
        "standard"
    }
}
