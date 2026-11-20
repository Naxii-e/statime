use statime::time::Time;
/// Software send timestamp fallback: when the socket cannot report when the packet
/// left, the best estimate is the clock read immediately after `send_to` returned.
pub fn resolve_send_timestamp(from_socket: Option<Time>, read_after_send: Time) -> Time {
    from_socket.unwrap_or(read_after_send)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefers_socket_timestamp() {
        let s = Time::from_nanos(100); let a = Time::from_nanos(200);
        assert_eq!(resolve_send_timestamp(Some(s), a), s);
    }
    #[test]
    fn falls_back_to_clock_read() {
        let a = Time::from_nanos(200);
        assert_eq!(resolve_send_timestamp(None, a), a);
    }
}
