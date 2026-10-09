use std::io::{self, BufRead, Read};
use std::thread;
use std::time::{Duration, Instant};

use stasis_network::client::{NetworkClient, STATUS_CONNECTED};

const HANDOFF_VERSION: &str = "stasis-network-supervision-v1\n";
const TIMEOUT: Duration = Duration::from_secs(90);
const MAGIC: i32 = 1_296_389_189;
const VERSION: i32 = 1;
const WORDS: usize = 36;
const WIRE_BYTES: usize = WORDS * 4;
const KIND_JOIN: i32 = 1;
const KIND_COMMAND: i32 = 2;
const KIND_ACK: i32 = 3;
const KIND_SNAPSHOT: i32 = 4;
const KIND_ERROR: i32 = 7;
const RESULT_ACCEPTED: i32 = 0;
const RESULT_SEQUENCE_REJECTED: i32 = 6;
const COMMAND_TTT_MOVE: i32 = 2;
const COMMAND_REMATCH_RESPONSE: i32 = 19;
const COMMAND_RETURN_TO_CATALOG: i32 = 20;

fn main() {
    if run().is_err() {
        std::process::exit(1);
    }
}

fn run() -> Result<(), ()> {
    let stdin = io::stdin();
    let mut input = stdin.lock().take(1024);
    let mut version = String::new();
    if input.read_line(&mut version).map_err(|_| ())? == 0 || version != HANDOFF_VERSION {
        return Err(());
    }
    let mut join_url = String::new();
    let length = input.read_line(&mut join_url).map_err(|_| ())?;
    if length < 2 || length > 513 || !join_url.ends_with('\n') || join_url.contains('\r') {
        return Err(());
    }
    join_url.pop();
    let mut trailing = [0_u8; 1];
    if input.read(&mut trailing).map_err(|_| ())? != 0 {
        return Err(());
    }

    let client = NetworkClient::new(&join_url).map_err(|_| ())?;
    join_url.clear();
    if client.connect() != 0 {
        return Err(());
    }
    let deadline = Instant::now() + TIMEOUT;
    while client.status() != STATUS_CONNECTED {
        if client.status() < 0 || Instant::now() >= deadline {
            return Err(());
        }
        thread::sleep(Duration::from_millis(2));
    }

    let join = Envelope::new(KIND_JOIN, 0, 0, -1, 0, &[]);
    send(&client, &join)?;
    println!("{{\"event\":\"sent_action\",\"sequence\":0,\"command\":0,\"arg\":0}}");

    let mut state = PeerState::default();
    let mut payload = [0_u8; WIRE_BYTES];
    while Instant::now() < deadline {
        let received = client.poll(&mut payload);
        if received < 0 {
            return Err(());
        }
        if received == 0 {
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        if received as usize != WIRE_BYTES {
            return Err(());
        }
        let envelope = Envelope::decode(&payload)?;
        match envelope.kind() {
            KIND_ACK => state.on_ack(&client, &envelope)?,
            KIND_SNAPSHOT => state.on_snapshot(&client, &envelope)?,
            KIND_ERROR => state.on_error(&envelope)?,
            _ => return Err(()),
        }
        if state.complete {
            if client.disconnect() != 0 {
                return Err(());
            }
            return Ok(());
        }
    }
    Err(())
}

#[derive(Clone)]
struct Envelope {
    words: [i32; WORDS],
}

impl Envelope {
    fn new(
        kind: i32,
        game_id: i32,
        connection: i32,
        seat: i32,
        sequence: i32,
        payload: &[i32],
    ) -> Self {
        let mut words = [0_i32; WORDS];
        words[0] = MAGIC;
        words[1] = VERSION;
        words[2] = kind;
        words[3] = game_id;
        words[4] = 0;
        words[5] = 1;
        words[6] = 1;
        words[7] = 1;
        words[8] = connection;
        words[9] = seat;
        words[10] = sequence;
        words[13] = payload.len() as i32;
        for (index, value) in payload.iter().enumerate() {
            words[18 + index] = *value;
        }
        words[35] = WORDS as i32;
        words[34] = checksum(&words);
        Self { words }
    }

    fn decode(bytes: &[u8]) -> Result<Self, ()> {
        if bytes.len() != WIRE_BYTES {
            return Err(());
        }
        let mut words = [0_i32; WORDS];
        for (index, chunk) in bytes.chunks_exact(4).enumerate() {
            words[index] = i32::from_be_bytes(chunk.try_into().map_err(|_| ())?);
        }
        if words[0] != MAGIC
            || words[1] != VERSION
            || !(0..=7).contains(&words[2])
            || !(0..=16).contains(&words[13])
            || words[35] != WORDS as i32
            || words[34] != checksum(&words)
        {
            return Err(());
        }
        for index in words[13] as usize..16 {
            if words[18 + index] != 0 {
                return Err(());
            }
        }
        Ok(Self { words })
    }

    fn encode(&self) -> [u8; WIRE_BYTES] {
        let mut bytes = [0_u8; WIRE_BYTES];
        for (index, word) in self.words.iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        bytes
    }

    fn kind(&self) -> i32 {
        self.words[2]
    }

    fn game_id(&self) -> i32 {
        self.words[3]
    }

    fn connection(&self) -> i32 {
        self.words[8]
    }

    fn seat(&self) -> i32 {
        self.words[9]
    }

    fn sequence(&self) -> i32 {
        self.words[10]
    }

    fn ack(&self) -> i32 {
        self.words[11]
    }

    fn revision(&self) -> i32 {
        self.words[16]
    }

    fn hash(&self) -> i32 {
        self.words[17]
    }

    fn payload_count(&self) -> usize {
        self.words[13] as usize
    }

    fn payload(&self, index: usize) -> i32 {
        self.words[18 + index]
    }
}

fn checksum(words: &[i32; WORDS]) -> i32 {
    let mut value = 29_i32;
    for word in &words[..18] {
        value = value.wrapping_mul(31).wrapping_add(*word);
    }
    for word in &words[18..34] {
        value = value.wrapping_mul(31).wrapping_add(*word);
    }
    value.wrapping_mul(31).wrapping_add(words[35])
}

fn send(client: &NetworkClient, envelope: &Envelope) -> Result<(), ()> {
    if client.send(&envelope.encode()) != 0 {
        return Err(());
    }
    Ok(())
}

fn send_action(
    client: &NetworkClient,
    connection: i32,
    sequence: i32,
    command: i32,
    arg: Option<i32>,
) -> Result<Envelope, ()> {
    let payload = match arg {
        Some(value) => vec![command, value],
        None => vec![command],
    };
    let envelope = Envelope::new(KIND_COMMAND, 1, connection, 1, sequence, &payload);
    send(client, &envelope)?;
    println!(
        "{{\"event\":\"sent_action\",\"sequence\":{},\"command\":{},\"arg\":{}}}",
        sequence,
        command,
        arg.unwrap_or(0)
    );
    Ok(envelope)
}

#[derive(Default)]
struct PeerState {
    joined: bool,
    connection: i32,
    malformed_sent: bool,
    malformed_rejected: bool,
    first_move_admitted: bool,
    duplicate_sent: bool,
    duplicate_rejected: bool,
    first_move_revision: i32,
    first_move_hash: i32,
    move_two_sent: bool,
    rematch_sent: bool,
    rematch_started: bool,
    draw_move_one_sent: bool,
    draw_move_two_sent: bool,
    draw_move_three_sent: bool,
    draw_move_four_sent: bool,
    return_sent: bool,
    complete: bool,
    pending_ack: Option<(i32, i32, i32)>,
    last_snapshot_revision: i32,
    last_snapshot_hash: i32,
}

impl PeerState {
    fn on_ack(&mut self, client: &NetworkClient, envelope: &Envelope) -> Result<(), ()> {
        if envelope.payload_count() != 2 || envelope.seat() != 1 {
            return Err(());
        }
        let result = envelope.payload(0);
        let revision = envelope.payload(1);
        if revision != envelope.revision() {
            return Err(());
        }
        if !self.joined {
            if envelope.sequence() != 0
                || envelope.ack() != 0
                || result != RESULT_ACCEPTED
                || envelope.connection() <= 0
            {
                return Err(());
            }
            self.joined = true;
            self.connection = envelope.connection();
            self.pending_ack = Some((0, revision, envelope.hash()));
            println!("{{\"event\":\"join_auth\",\"seat\":1,\"ack\":0}}");
            return Ok(());
        }

        if envelope.sequence() == 1 && result == RESULT_ACCEPTED && !self.first_move_admitted {
            if envelope.ack() != 1 {
                return Err(());
            }
            self.first_move_admitted = true;
            self.first_move_revision = revision;
            self.first_move_hash = envelope.hash();
            self.pending_ack = Some((1, revision, envelope.hash()));
            println!(
                "{{\"event\":\"ack\",\"sequence\":1,\"ack\":1,\"result\":0,\"revision\":{},\"hash\":{}}}",
                revision,
                envelope.hash()
            );
            let duplicate = Envelope::new(
                KIND_COMMAND,
                1,
                self.connection,
                1,
                1,
                &[COMMAND_TTT_MOVE, 3],
            );
            send(client, &duplicate)?;
            self.duplicate_sent = true;
            println!("{{\"event\":\"sent_action\",\"sequence\":1,\"command\":2,\"arg\":3}}");
            return Ok(());
        }

        if envelope.sequence() == 1 && result == RESULT_SEQUENCE_REJECTED && self.duplicate_sent {
            if envelope.ack() != 1
                || revision != self.first_move_revision
                || envelope.hash() != self.first_move_hash
            {
                return Err(());
            }
            self.duplicate_rejected = true;
            println!(
                "{{\"event\":\"duplicate_rejected\",\"sequence\":1,\"ack\":1,\"result\":6,\"revision\":{},\"hash\":{}}}",
                revision,
                envelope.hash()
            );
            return Ok(());
        }

        if result != RESULT_ACCEPTED || envelope.ack() != envelope.sequence() {
            return Err(());
        }
        self.pending_ack = Some((envelope.sequence(), revision, envelope.hash()));
        println!(
            "{{\"event\":\"ack\",\"sequence\":{},\"ack\":{},\"result\":0,\"revision\":{},\"hash\":{}}}",
            envelope.sequence(),
            envelope.ack(),
            revision,
            envelope.hash()
        );
        Ok(())
    }

    fn on_error(&mut self, envelope: &Envelope) -> Result<(), ()> {
        if envelope.payload_count() != 1
            || envelope.payload(0) != 7
            || envelope.revision() != self.last_snapshot_revision
            || envelope.hash() != self.last_snapshot_hash
            || envelope.ack() != expected_sequence(self.last_snapshot_revision)
        {
            return Err(());
        }
        self.malformed_rejected = true;
        println!(
            "{{\"event\":\"malformed_rejected\",\"code\":7,\"revision\":{},\"hash\":{}}}",
            envelope.revision(),
            envelope.hash()
        );
        Ok(())
    }

    fn on_snapshot(&mut self, client: &NetworkClient, envelope: &Envelope) -> Result<(), ()> {
        if !self.joined
            || envelope.payload_count() != 16
            || envelope.words[14] != 0
            || envelope.words[15] != 1
            || envelope.payload(0) != 1
        {
            return Err(());
        }
        let phase = envelope.payload(13);
        let terminal_phase = envelope.payload(14);
        let screen = envelope.payload(15);
        let current = envelope.payload(10);
        let winner = envelope.payload(11);
        let moves = envelope.payload(12);
        let revision = envelope.revision();
        let mut projection = [0_i32; 16];
        for (index, word) in projection.iter_mut().enumerate() {
            *word = envelope.payload(index);
        }
        if envelope.hash() != projection_hash(&projection)
            || Some(projection) != expected_projection(revision)
            || envelope.game_id() != if screen == 0 { 0 } else { 1 }
            || envelope.sequence() != expected_sequence(revision)
            || envelope.ack() != expected_sequence(revision)
            || revision != self.last_snapshot_revision + 1
        {
            return Err(());
        }
        if let Some(sequence) = expected_ack_sequence(revision) {
            if self.pending_ack != Some((sequence, revision, envelope.hash())) {
                return Err(());
            }
            self.pending_ack = None;
        } else if self.pending_ack.is_some() {
            return Err(());
        }
        self.last_snapshot_revision = revision;
        self.last_snapshot_hash = envelope.hash();
        println!(
            "{{\"event\":\"snapshot\",\"game_id\":{},\"phase\":{},\"terminal_phase\":{},\"screen\":{},\"revision\":{},\"hash\":{},\"sequence\":{},\"current\":{},\"winner\":{},\"moves\":{}}}",
            envelope.game_id(),
            phase,
            terminal_phase,
            screen,
            envelope.revision(),
            envelope.hash(),
            envelope.ack(),
            current,
            winner,
            moves
        );

        if screen == 0 && self.return_sent && terminal_phase == 10 {
            if !self.malformed_rejected || !self.duplicate_rejected {
                return Err(());
            }
            println!(
                "{{\"event\":\"complete\",\"revision\":{},\"hash\":{},\"sequence\":{}}}",
                revision,
                envelope.hash(),
                envelope.ack()
            );
            self.complete = true;
            return Ok(());
        }

        if screen != 1 || envelope.game_id() != 1 {
            return Ok(());
        }

        if !self.rematch_started
            && phase == 0
            && current == 1
            && moves == 1
            && envelope.payload(1) == 1
        {
            if !self.malformed_sent {
                let valid = Envelope::new(
                    KIND_COMMAND,
                    1,
                    self.connection,
                    1,
                    1,
                    &[COMMAND_TTT_MOVE, 3],
                );
                let mut malformed = valid.encode();
                malformed[WIRE_BYTES - 8] ^= 1;
                if client.send(&malformed) != 0 {
                    return Err(());
                }
                self.malformed_sent = true;
                println!("{{\"event\":\"sent_malformed\",\"sequence\":1,\"kind\":2}}");
                send(client, &valid)?;
                println!("{{\"event\":\"sent_action\",\"sequence\":1,\"command\":2,\"arg\":3}}");
            }
        } else if !self.rematch_started
            && phase == 0
            && current == 1
            && moves == 3
            && envelope.payload(2) == 1
            && envelope.payload(4) == 2
            && !self.move_two_sent
        {
            let sent = send_action(client, self.connection, 2, COMMAND_TTT_MOVE, Some(4))?;
            self.move_two_sent = sent.sequence() == 2;
        } else if !self.rematch_started && phase == 1 && terminal_phase == 2 && !self.rematch_sent {
            let sent = send_action(
                client,
                self.connection,
                3,
                COMMAND_REMATCH_RESPONSE,
                Some(1),
            )?;
            self.rematch_sent = sent.sequence() == 3;
        } else if self.rematch_sent && phase == 0 && moves == 0 && terminal_phase == 7 {
            self.rematch_started = true;
        } else if self.rematch_started
            && phase == 0
            && current == 1
            && moves == 1
            && !self.draw_move_one_sent
        {
            let sent = send_action(client, self.connection, 4, COMMAND_TTT_MOVE, Some(1))?;
            self.draw_move_one_sent = sent.sequence() == 4;
        } else if self.rematch_started
            && phase == 0
            && current == 1
            && moves == 3
            && !self.draw_move_two_sent
        {
            let sent = send_action(client, self.connection, 5, COMMAND_TTT_MOVE, Some(3))?;
            self.draw_move_two_sent = sent.sequence() == 5;
        } else if self.rematch_started
            && phase == 0
            && current == 1
            && moves == 5
            && !self.draw_move_three_sent
        {
            let sent = send_action(client, self.connection, 6, COMMAND_TTT_MOVE, Some(4))?;
            self.draw_move_three_sent = sent.sequence() == 6;
        } else if self.rematch_started
            && phase == 0
            && current == 1
            && moves == 7
            && !self.draw_move_four_sent
        {
            let sent = send_action(client, self.connection, 7, COMMAND_TTT_MOVE, Some(8))?;
            self.draw_move_four_sent = sent.sequence() == 7;
        } else if self.rematch_started && phase == 2 && terminal_phase == 1 && !self.return_sent {
            let sent = send_action(client, self.connection, 8, COMMAND_RETURN_TO_CATALOG, None)?;
            self.return_sent = sent.sequence() == 8;
        }
        Ok(())
    }
}

fn projection_hash(projection: &[i32; 16]) -> i32 {
    let mut hash = 17_i32;
    for word in projection {
        hash = hash.wrapping_mul(31).wrapping_add(*word);
    }
    hash
}

fn expected_projection(revision: i32) -> Option<[i32; 16]> {
    match revision {
        1 => Some([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -1, 0, 0, 0, 0]),
        2 => Some([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -1, 0, 0, 0, 1]),
        3 => Some([1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, -1, 1, 0, 0, 1]),
        4 => Some([1, 1, 0, 0, 2, 0, 0, 0, 0, 0, 0, -1, 2, 0, 0, 1]),
        5 => Some([1, 1, 1, 0, 2, 0, 0, 0, 0, 0, 1, -1, 3, 0, 0, 1]),
        6 => Some([1, 1, 1, 0, 2, 2, 0, 0, 0, 0, 0, -1, 4, 0, 0, 1]),
        7 => Some([1, 1, 1, 1, 2, 2, 0, 0, 0, 0, 0, 0, 5, 1, 1, 1]),
        8 => Some([1, 1, 1, 1, 2, 2, 0, 0, 0, 0, 0, 0, 5, 1, 2, 1]),
        9 => Some([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -1, 0, 0, 7, 1]),
        10 => Some([1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, -1, 1, 0, 7, 1]),
        11 => Some([1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, -1, 2, 0, 7, 1]),
        12 => Some([1, 1, 2, 1, 0, 0, 0, 0, 0, 0, 1, -1, 3, 0, 7, 1]),
        13 => Some([1, 1, 2, 1, 2, 0, 0, 0, 0, 0, 0, -1, 4, 0, 7, 1]),
        14 => Some([1, 1, 2, 1, 2, 0, 0, 1, 0, 0, 1, -1, 5, 0, 7, 1]),
        15 => Some([1, 1, 2, 1, 2, 2, 0, 1, 0, 0, 0, -1, 6, 0, 7, 1]),
        16 => Some([1, 1, 2, 1, 2, 2, 0, 1, 1, 0, 1, -1, 7, 0, 7, 1]),
        17 => Some([1, 1, 2, 1, 2, 2, 0, 1, 1, 2, 0, -1, 8, 0, 7, 1]),
        18 => Some([1, 1, 2, 1, 2, 2, 1, 1, 1, 2, 0, -1, 9, 2, 1, 1]),
        19 => Some([1, 1, 2, 1, 2, 2, 1, 1, 1, 2, 0, -1, 9, 2, 8, 1]),
        20 => Some([1, 1, 2, 1, 2, 2, 1, 1, 1, 2, 0, -1, 9, 2, 10, 0]),
        _ => None,
    }
}

fn expected_sequence(revision: i32) -> i32 {
    match revision {
        1..=3 => 0,
        4..=5 => 1,
        6..=8 => 2,
        9..=10 => 3,
        11..=12 => 4,
        13..=14 => 5,
        15..=16 => 6,
        17..=18 => 7,
        19..=20 => 8,
        _ => -1,
    }
}

fn expected_ack_sequence(revision: i32) -> Option<i32> {
    match revision {
        1 => Some(0),
        4 => Some(1),
        6 => Some(2),
        9 => Some(3),
        11 => Some(4),
        13 => Some(5),
        15 => Some(6),
        17 => Some(7),
        19 => Some(8),
        _ => None,
    }
}
