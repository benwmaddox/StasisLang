export function fakeWebGL2(overrides = {}) {
  let id = 1;
  const object = () => ({ id: id++ });
  return {
    VERTEX_SHADER: 1, FRAGMENT_SHADER: 2, COMPILE_STATUS: 3, LINK_STATUS: 4,
    ARRAY_BUFFER: 5, STATIC_DRAW: 6, DYNAMIC_DRAW: 7, FLOAT: 8,
    TEXTURE_2D: 9, TEXTURE_WRAP_S: 10, TEXTURE_WRAP_T: 11,
    CLAMP_TO_EDGE: 12, TEXTURE_MIN_FILTER: 13, TEXTURE_MAG_FILTER: 14,
    LINEAR: 15, RGBA: 16, UNSIGNED_BYTE: 17, NO_ERROR: 0,
    BLEND: 18, SRC_ALPHA: 19, ONE_MINUS_SRC_ALPHA: 20, ONE: 21,
    TRIANGLE_STRIP: 22, TEXTURE0: 23, COLOR_BUFFER_BIT: 24,
    SCISSOR_TEST: 25, MAX_TEXTURE_SIZE: 26,
    createShader: object, createProgram: object, shaderSource() {}, compileShader() {},
    getShaderParameter: () => true, getShaderInfoLog: () => "", attachShader() {},
    linkProgram() {}, getProgramParameter: () => true, getProgramInfoLog: () => "",
    createVertexArray: object, createBuffer: object, bindBuffer() {}, bufferData() {},
    bindVertexArray() {}, enableVertexAttribArray() {}, vertexAttribPointer() {},
    vertexAttribDivisor() {}, getUniformLocation: object, createTexture: object,
    bindTexture() {}, texParameteri() {}, texImage2D() {}, texSubImage2D() {},
    getError: () => 0, isContextLost: () => false, deleteTexture() {}, deleteBuffer() {},
    deleteVertexArray() {}, deleteProgram() {}, viewport() {}, useProgram() {},
    uniform2f() {}, uniform1i() {}, bufferSubData() {}, enable() {}, disable() {},
    blendFuncSeparate() {}, activeTexture() {}, drawArraysInstanced() {}, clearColor() {},
    clear() {}, scissor() {}, getParameter: parameter => parameter === 26 ? 4096 : 0,
    ...overrides
  };
}
