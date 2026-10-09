export const DEFAULT_PONG_SOURCE = `import "vendor/stasis/stdlib/graphics.stasis";
import "vendor/stasis/stdlib/host_frame.stasis";

const SCREEN_WIDTH: f32 = 640.0;
const SCREEN_HEIGHT: f32 = 360.0;
const PADDLE_X: f32 = 30.0;
const CPU_PADDLE_X: f32 = 598.0;
const PADDLE_HEIGHT: f32 = 72.0;
const BALL_RADIUS: f32 = 7.0;
const WIN_SCORE: i32 = 5;

// HostFrame.keys uses SDL scancode slots.
const SCANCODE_W: i32 = 26;
const SCANCODE_S: i32 = 22;
const SCANCODE_SPACE: i32 = 44;
const SCANCODE_UP: i32 = 82;
const SCANCODE_DOWN: i32 = 81;

struct BallState {
    sprite: Sprite;
    x: f32;
    y: f32;
    vx: f32;
    vy: f32;
}

struct PaddleState {
    y: f32;
    score: i32;
    score_text: TextRun;
}

struct PongState {
    input_frame: HostFrame;
    arena: Sprite;
    paddle: Sprite;
    ball: BallState;
    player: PaddleState;
    cpu: PaddleState;
    score_font: i32;
    score_utf8: utf8[12];
    game_over: bool;
}

global state: PongState;

function refresh_scores(): void {
    state.score_utf8.from_i32(state.player.score);
    state.player.score_text.replace_text_from(state.score_font, state.score_utf8);
    state.score_utf8.from_i32(state.cpu.score);
    state.cpu.score_text.replace_text_from(state.score_font, state.score_utf8);
}

function serve(direction: i32): void {
    state.ball.x = SCREEN_WIDTH / 2.0;
    state.ball.y = SCREEN_HEIGHT / 2.0;
    state.ball.vx = 4.0;
    state.ball.vy = 2.25;
    if (direction < 0) {
        state.ball.vx = 0.0 - state.ball.vx;
    }
}

function reset_match(): void {
    state.player.y = 144.0;
    state.cpu.y = 144.0;
    state.player.score = 0;
    state.cpu.score = 0;
    state.game_over = false;
    serve(1);
    refresh_scores();
}

function main(): i32 {
    init_window(640, 360, "Stasis Pong");
    state.arena.load_sprite_from("assets/pong-arena.png", 640, 360);
    state.paddle.load_sprite_from("assets/pong-paddle.png", 12, 72);
    state.ball.sprite.load_sprite_from("assets/pong-ball.png", 16, 16);
    state.score_font = load_font("assets/ui.ttf", 18);
    reset_match();
    return 0;
}

function clamp_paddle(y: f32): f32 {
    if (y < 20.0) {
        return 20.0;
    }
    if (y > 268.0) {
        return 268.0;
    }
    return y;
}

function move_player(): void {
    if (state.input_frame.pointer_count > 0 && state.input_frame.pointers[0].is_down) {
        state.player.y = state.input_frame.pointers[0].y_logical - PADDLE_HEIGHT / 2.0;
    } else if (state.input_frame.keys[SCANCODE_UP] != 0 || state.input_frame.keys[SCANCODE_W] != 0) {
        state.player.y -= 5.0;
    } else if (state.input_frame.keys[SCANCODE_DOWN] != 0 || state.input_frame.keys[SCANCODE_S] != 0) {
        state.player.y += 5.0;
    }
    state.player.y = clamp_paddle(state.player.y);
}

function move_cpu(): void {
    let cpu_center: f32 = state.cpu.y + PADDLE_HEIGHT / 2.0;
    if (state.ball.y < cpu_center) {
        state.cpu.y -= 2.4;
    } else if (state.ball.y > cpu_center) {
        state.cpu.y += 2.4;
    }
    state.cpu.y = clamp_paddle(state.cpu.y);
}

function bounce_from_paddle(paddle_y: f32, contact_x: f32): void {
    state.ball.x = contact_x;
    state.ball.vx = 0.0 - state.ball.vx;
    if (state.ball.y < paddle_y + PADDLE_HEIGHT / 2.0) {
        state.ball.vy -= 1.0;
    } else if (state.ball.y > paddle_y + PADDLE_HEIGHT / 2.0) {
        state.ball.vy += 1.0;
    }
}

function move_ball(): void {
    state.ball.x += state.ball.vx;
    state.ball.y += state.ball.vy;

    if (state.ball.y < 18.0) {
        state.ball.y = 18.0;
        state.ball.vy = 0.0 - state.ball.vy;
    } else if (state.ball.y > 342.0) {
        state.ball.y = 342.0;
        state.ball.vy = 0.0 - state.ball.vy;
    }

    if (state.ball.vx < 0.0 && state.ball.x <= 49.0 && state.ball.x >= 23.0
        && state.ball.y + BALL_RADIUS >= state.player.y
        && state.ball.y - BALL_RADIUS <= state.player.y + PADDLE_HEIGHT) {
        bounce_from_paddle(state.player.y, 49.0);
    }
    if (state.ball.vx > 0.0 && state.ball.x >= 591.0 && state.ball.x <= 617.0
        && state.ball.y + BALL_RADIUS >= state.cpu.y
        && state.ball.y - BALL_RADIUS <= state.cpu.y + PADDLE_HEIGHT) {
        bounce_from_paddle(state.cpu.y, 591.0);
    }

    if (state.ball.vy > 5.0) {
        state.ball.vy = 5.0;
    } else if (state.ball.vy < -5.0) {
        state.ball.vy = -5.0;
    }
}

function check_score(): void {
    if (state.ball.x < 0.0 - BALL_RADIUS - 1.0) {
        state.cpu.score += 1;
        refresh_scores();
        if (state.cpu.score >= WIN_SCORE) {
            state.game_over = true;
        } else {
            serve(1);
        }
    } else if (state.ball.x > SCREEN_WIDTH + BALL_RADIUS + 1.0) {
        state.player.score += 1;
        refresh_scores();
        if (state.player.score >= WIN_SCORE) {
            state.game_over = true;
        } else {
            serve(-1);
        }
    }
}

function tick(): i32 {
    state.input_frame.refresh();
    if (state.game_over) {
        if (state.input_frame.keys[SCANCODE_SPACE] != 0
            || (state.input_frame.pointer_count > 0 && state.input_frame.pointers[0].went_down)) {
            reset_match();
        }
        return 0;
    }

    move_player();
    move_cpu();
    move_ball();
    check_score();
    return 0;
}

function render(): i32 {
    state.arena.draw(0.0, 0.0, 255, 0);
    state.paddle.draw(PADDLE_X, state.player.y, 255, 0);
    state.paddle.draw(CPU_PADDLE_X, state.cpu.y, 255, 0);
    state.ball.sprite.draw(state.ball.x - 8.0, state.ball.y - 8.0, 255, 0);
    state.player.score_text.draw(280.0, 24.0, 0.34, 0.91, 0.80, 1.0);
    state.cpu.score_text.draw(344.0, 24.0, 1.0, 0.75, 0.38, 1.0);

    if (state.game_over) {
        if (state.player.score >= WIN_SCORE) {
            state.score_font.draw_text("YOU WIN", 257.0, 150.0, 0.34, 0.91, 0.80, 1.0);
        } else {
            state.score_font.draw_text("CPU WINS", 248.0, 150.0, 1.0, 0.75, 0.38, 1.0);
        }
        state.score_font.draw_text("SPACE OR TAP TO RESTART", 113.0, 194.0, 0.96, 0.93, 0.82, 1.0);
    }
    return 0;
}
`;

export const DEFAULT_PONG_IMAGES = Object.freeze([
  {
    name: "pong-arena.svg",
    path: "assets/pong-arena.png",
    source: `<svg xmlns="http://www.w3.org/2000/svg" width="640" height="360" viewBox="0 0 640 360">
  <rect width="640" height="360" fill="#07141d"/>
  <rect x="12" y="12" width="616" height="336" rx="18" fill="#0a202b" stroke="#28525d" stroke-width="2"/>
  <rect x="26" y="28" width="588" height="304" rx="10" fill="#0b1b25" stroke="#173d49" stroke-width="2"/>
  <circle cx="320" cy="180" r="56" fill="none" stroke="#1d4550" stroke-width="2"/>
  <line x1="320" y1="36" x2="320" y2="324" stroke="#32616b" stroke-width="4" stroke-dasharray="10 14" stroke-linecap="round"/>
  <line x1="44" y1="42" x2="100" y2="42" stroke="#1b404b" stroke-width="2"/>
  <line x1="540" y1="318" x2="596" y2="318" stroke="#1b404b" stroke-width="2"/>
</svg>`,
  },
  {
    name: "pong-paddle.svg",
    path: "assets/pong-paddle.png",
    source: `<svg xmlns="http://www.w3.org/2000/svg" width="12" height="72" viewBox="0 0 12 72">
  <rect x="1" y="1" width="10" height="70" rx="5" fill="#54ead4"/>
  <rect x="3" y="7" width="2" height="58" rx="1" fill="#d2fff6" opacity="0.55"/>
</svg>`,
  },
  {
    name: "pong-ball.svg",
    path: "assets/pong-ball.png",
    source: `<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">
  <circle cx="8" cy="8" r="6.5" fill="#f6ca74" stroke="#fff0bd" stroke-width="1.5"/>
  <circle cx="6" cy="6" r="1.5" fill="#fff8df"/>
</svg>`,
  },
]);

export async function importDefaultPongImages(assetStore) {
  const imported = [];
  for (const asset of DEFAULT_PONG_IMAGES) {
    const file = new File([asset.source], asset.name, { type: "image/svg+xml" });
    const result = await assetStore.importFile(file);
    if (result.path !== asset.path) {
      throw new Error(`default Pong image ${asset.name} imported to unexpected path ${result.path}`);
    }
    imported.push(result);
  }
  return imported;
}
