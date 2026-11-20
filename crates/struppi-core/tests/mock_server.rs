//! End-to-end tests of the network task and the session against an
//! in-process mock telnet server.

use std::time::Duration;

use struppi_core::{Connection, NetEvent, Session};
use struppi_telnet::consts::{TTYPE_IS, TTYPE_SEND, cmd::*, opt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(5);

/// Binds a listener on a free local port.
async fn listen() -> (TcpListener, u16) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// Reads from `stream` until `needle` has been seen.
async fn read_until(stream: &mut TcpStream, needle: &[u8]) -> Vec<u8> {
    let mut seen = Vec::new();
    let mut buffer = [0u8; 1024];
    while !seen.windows(needle.len()).any(|window| window == needle) {
        let count = tokio::time::timeout(TIMEOUT, stream.read(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert!(count > 0, "client closed the connection; got {seen:?}");
        seen.extend_from_slice(&buffer[..count]);
    }
    seen
}

/// Drives a session from network events until `done` holds.
async fn drive(
    session: &mut Session,
    connection: &Connection,
    events: &mut mpsc::Receiver<NetEvent>,
    done: impl Fn(&Session) -> bool,
) {
    while !done(session) {
        match tokio::time::timeout(TIMEOUT, events.recv()).await.unwrap() {
            Some(NetEvent::Data(bytes)) => {
                session.receive(&bytes);
                connection.send(session.take_output());
            }
            Some(NetEvent::Connected) => {}
            other => panic!("unexpected event: {other:?}"),
        }
    }
}

fn texts(session: &Session) -> Vec<String> {
    let scrollback = session.scrollback();
    (scrollback.first_seq()..scrollback.end_seq())
        .filter_map(|seq| scrollback.get(seq))
        .map(|line| line.text.clone())
        .collect()
}

#[tokio::test]
async fn negotiates_and_exchanges_lines() {
    let (listener, port) = listen().await;

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream
            .write_all(&[
                IAC,
                DO,
                opt::NAWS,
                IAC,
                DO,
                opt::TTYPE,
                IAC,
                WILL,
                opt::MCCP2,
                IAC,
                WILL,
                opt::EOR,
            ])
            .await
            .unwrap();
        stream
            .write_all(b"Welcome to \x1b[1;32mMock\x1b[0m!\r\nName: ")
            .await
            .unwrap();
        stream.write_all(&[IAC, EOR]).await.unwrap();

        // NAWS 100x30, refusal of MCCP2, acceptance of EOR.
        let naws = [IAC, SB, opt::NAWS, 0, 100, 0, 30, IAC, SE];
        let seen = read_until(&mut stream, b"tester\r\n").await;
        for expected in [
            &[IAC, WILL, opt::NAWS][..],
            &naws,
            &[IAC, WILL, opt::TTYPE],
            &[IAC, DONT, opt::MCCP2],
            &[IAC, DO, opt::EOR],
        ] {
            assert!(
                seen.windows(expected.len()).any(|w| w == expected),
                "missing {expected:?} in {seen:?}"
            );
        }

        stream
            .write_all(&[IAC, SB, opt::TTYPE, TTYPE_SEND, IAC, SE])
            .await
            .unwrap();
        let mut answer = vec![IAC, SB, opt::TTYPE, TTYPE_IS];
        answer.extend_from_slice(b"STRUPPI");
        answer.extend_from_slice(&[IAC, SE]);
        let seen = read_until(&mut stream, &[IAC, SE]).await;
        assert!(
            seen.windows(answer.len()).any(|w| w == answer),
            "bad TTYPE answer: {seen:?}"
        );

        stream.write_all(b"Hello, tester.\r\n").await.unwrap();
        // Keep the socket open until the client is done.
        let _ = stream.read(&mut [0u8; 16]).await;
    });

    let (tx, mut events) = mpsc::channel(16);
    let connection = Connection::open("127.0.0.1".to_owned(), port, tx);
    let mut session = Session::new();
    session.resize(100, 30);

    assert_eq!(
        tokio::time::timeout(TIMEOUT, events.recv()).await.unwrap(),
        Some(NetEvent::Connected)
    );
    drive(&mut session, &connection, &mut events, |s| {
        s.partial().is_some_and(|p| p.prompt)
    })
    .await;
    assert_eq!(texts(&session), ["Welcome to Mock!"]);
    assert_eq!(session.partial().unwrap().text, "Name: ");

    session.send_line("tester");
    connection.send(session.take_output());
    drive(&mut session, &connection, &mut events, |s| {
        s.scrollback().len() == 3
    })
    .await;
    assert_eq!(
        texts(&session),
        ["Welcome to Mock!", "Name: tester", "Hello, tester."]
    );

    drop(connection);
    tokio::time::timeout(TIMEOUT, server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn reports_server_close() {
    let (listener, port) = listen().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(b"bye\r\n").await.unwrap();
    });

    let (tx, mut events) = mpsc::channel(16);
    let _connection = Connection::open("127.0.0.1".to_owned(), port, tx);

    let mut received = Vec::new();
    loop {
        match tokio::time::timeout(TIMEOUT, events.recv()).await.unwrap() {
            Some(NetEvent::Connected) => {}
            Some(NetEvent::Data(bytes)) => received.extend(bytes),
            Some(NetEvent::Closed(reason)) => {
                assert_eq!(reason, None);
                break;
            }
            None => panic!("channel closed without a Closed event"),
        }
    }
    assert_eq!(received, b"bye\r\n");
    server.await.unwrap();
}

#[tokio::test]
async fn reports_connection_failure() {
    // Bind and drop to get a port nothing listens on.
    let (listener, port) = listen().await;
    drop(listener);

    let (tx, mut events) = mpsc::channel(16);
    let _connection = Connection::open("127.0.0.1".to_owned(), port, tx);
    match tokio::time::timeout(TIMEOUT, events.recv()).await.unwrap() {
        Some(NetEvent::Closed(Some(reason))) => assert!(!reason.is_empty()),
        other => panic!("unexpected event: {other:?}"),
    }
}
