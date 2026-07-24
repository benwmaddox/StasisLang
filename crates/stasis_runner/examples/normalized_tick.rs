use stasis_runner::tick::{BoundedEventQueue, BoundedPool, TickCoordinator, TickTransaction};

#[derive(Clone)]
struct Simulation {
    enemies: BoundedPool<i32, 6>,
    effects: BoundedPool<i32, 4>,
    target: Option<usize>,
    events: BoundedEventQueue<&'static str, 4>,
}

fn state_hash(state: &Simulation) -> u64 {
    state
        .enemies
        .items()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, value| {
            (hash ^ (*value as u64)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

struct SimulationTick<'a> {
    state: &'a mut Simulation,
}

impl TickTransaction for SimulationTick<'_> {
    type Checkpoint = Simulation;
    type Snapshot = (Vec<i32>, Vec<i32>, Option<usize>);

    fn checkpoint(&mut self) -> Result<Self::Checkpoint, String> {
        Ok(self.state.clone())
    }

    fn restore(&mut self, checkpoint: Self::Checkpoint) {
        *self.state = checkpoint;
    }

    fn gameplay(&mut self) -> Result<(), String> {
        self.state.enemies.queue_destroy(1)?;
        self.state.enemies.queue_destroy(3)?;
        self.state.enemies.queue_spawn(50)?;
        self.state.enemies.queue_spawn(60)?;
        self.state.effects.queue_destroy(0)?;
        self.state.effects.queue_spawn(9)?;
        self.state.events.emit("destroyed:1")?;
        self.state.events.emit("destroyed:3")?;
        Ok(())
    }

    fn commit_structural(&mut self) -> Result<(), String> {
        let enemy_commit = self.state.enemies.commit()?;
        enemy_commit.repair_optional(&mut self.state.target);
        self.state.effects.commit()?;
        self.state.events.commit()?;
        Ok(())
    }

    fn normalize(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn validate(&mut self) -> Result<(), String> {
        if self
            .state
            .target
            .is_some_and(|index| index >= self.state.enemies.items().len())
        {
            return Err("target index was not repaired".to_string());
        }
        Ok(())
    }

    fn state_hash(&mut self) -> Result<u64, String> {
        Ok(state_hash(self.state))
    }

    fn capture(&mut self) -> Result<Self::Snapshot, String> {
        Ok((
            self.state.enemies.items().to_vec(),
            self.state.effects.items().to_vec(),
            self.state.target,
        ))
    }

    fn render(&mut self) -> Result<(), String> {
        Ok(())
    }
}

fn main() -> Result<(), String> {
    let mut simulation = Simulation {
        enemies: BoundedPool::from_items(vec![10, 20, 30, 40])?,
        effects: BoundedPool::from_items(vec![7, 8])?,
        target: Some(2),
        events: BoundedEventQueue::default(),
    };
    let mut ticks = TickCoordinator::default();

    let commit = ticks
        .run_tick(&mut SimulationTick {
            state: &mut simulation,
        })
        .map_err(|error| error.to_string())?;

    assert_eq!(commit.snapshot, (vec![10, 30, 50, 60], vec![8, 9], Some(1)));
    assert_eq!(
        simulation.events.drain_committed().collect::<Vec<_>>(),
        vec!["destroyed:1", "destroyed:3"]
    );
    println!(
        "tick={} enemies={:?} target={:?} hash={:016x}",
        commit.tick,
        simulation.enemies.items(),
        simulation.target,
        commit.state_hash
    );
    Ok(())
}
