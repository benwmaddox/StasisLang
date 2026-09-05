const SCREEN_WIDTH = 640;
const SCREEN_HEIGHT = 360;
const PADDLE_WIDTH = 12;
const PADDLE_HEIGHT = 72;
const BALL_SIZE = 12;

const canvas = document.querySelector("#pong");
const context = canvas.getContext("2d", { alpha: false });

const game = {
  leftY: 144,
  rightY: 144,
  ballX: 0,
  ballY: 0,
  ballDX: 0,
  ballDY: 0,
  leftScore: 0,
  rightScore: 0,
};

function resetBall(direction) {
  game.ballX = SCREEN_WIDTH / 2 - BALL_SIZE / 2;
  game.ballY = SCREEN_HEIGHT / 2 - BALL_SIZE / 2;
  game.ballDX = direction * 4;
  game.ballDY = 3;
}

function followBall(y) {
  const center = y + PADDLE_HEIGHT / 2;
  if (center < game.ballY) {
    y += 3;
  } else if (center > game.ballY + BALL_SIZE) {
    y -= 3;
  }
  return Math.max(0, Math.min(y, SCREEN_HEIGHT - PADDLE_HEIGHT));
}

function update() {
  game.leftY = followBall(game.leftY);
  game.rightY = followBall(game.rightY);
  game.ballX += game.ballDX;
  game.ballY += game.ballDY;

  if (game.ballY <= 0 || game.ballY >= SCREEN_HEIGHT - BALL_SIZE) {
    game.ballDY = -game.ballDY;
  }
  if (
    game.ballX >= 20 &&
    game.ballX <= 32 &&
    game.ballY + BALL_SIZE >= game.leftY &&
    game.ballY <= game.leftY + PADDLE_HEIGHT
  ) {
    game.ballDX = 4;
  }
  if (
    game.ballX + BALL_SIZE >= 608 &&
    game.ballX <= 620 &&
    game.ballY + BALL_SIZE >= game.rightY &&
    game.ballY <= game.rightY + PADDLE_HEIGHT
  ) {
    game.ballDX = -4;
  }
  if (game.ballX < 0) {
    game.rightScore += 1;
    resetBall(1);
  }
  if (game.ballX > SCREEN_WIDTH) {
    game.leftScore += 1;
    resetBall(-1);
  }
}

function rectangle(x, y, width, height, color) {
  context.fillStyle = color;
  context.fillRect(x, y, width, height);
}

function render() {
  rectangle(0, 0, SCREEN_WIDTH, SCREEN_HEIGHT, "rgb(5 12 24)");
  rectangle(20, game.leftY, PADDLE_WIDTH, PADDLE_HEIGHT, "rgb(83 216 251)");
  rectangle(608, game.rightY, PADDLE_WIDTH, PADDLE_HEIGHT, "rgb(251 180 83)");
  rectangle(game.ballX, game.ballY, BALL_SIZE, BALL_SIZE, "rgb(245 248 255)");
  rectangle(318, 0, 4, SCREEN_HEIGHT, "rgb(42 76 102)");

  context.fillStyle = "#dff6ff";
  context.font = "18px ui-monospace, Consolas, monospace";
  context.fillText(`score ${game.leftScore}`, 250, 30);
  context.fillText(`score ${game.rightScore}`, 370, 30);
}

let frames = 0;

function frame() {
  update();
  render();
  frames += 1;
  document.body.dataset.frames = String(frames);
  requestAnimationFrame(frame);
}

resetBall(1);
document.body.dataset.ready = "true";
document.body.dataset.runtime = "javascript";
requestAnimationFrame(frame);
