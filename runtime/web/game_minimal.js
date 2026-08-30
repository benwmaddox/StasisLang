(() => {
  "use strict";
  const canvas = document.getElementById("stasis-canvas");
  const context = canvas.getContext("2d", { alpha: false });
  const hud = document.getElementById("stasis-hud");
  const errorBox = document.getElementById("stasis-error");
  const loadingBox = document.getElementById("stasis-loading");
  const loadingStatus = document.getElementById("stasis-loading-status");
  const setLoading = (message, state = "loading") => {
    if (!loadingBox) return;
    if (loadingStatus) loadingStatus.textContent = message;
    else loadingBox.textContent = message;
    loadingBox.dataset.failed = state === "failed" ? "true" : "false";
    loadingBox.dataset.hidden = state === "ready" ? "true" : "false";
  };
  const game = window.STASIS_GAME || { strings: {} };
  const commands = [];
  const stringValue = id => game.strings[String(id)] || "";
  const color = (r, g, b) => `rgb(${r & 255} ${g & 255} ${b & 255})`;
  const imports = { env: {
__STASIS_IMPORTS__
  }};
  let instance;
  let frames = 0;
  let worstTick = 0;
  let worstRender = 0;
  let worstWasmRender=0,worstBrowserReplay=0,worstFrameWork=0;
  const PERF_ROLLING_CAPACITY=1200;
  const worstTimes=new Float64Array(PERF_ROLLING_CAPACITY);
  const worstValues=Array.from({length:5},()=>new Float64Array(PERF_ROLLING_CAPACITY));
  let worstNext=0,worstCount=0;
  const recordWorst=(now,tick,render,wasm,replay,frameWork)=>{
    worstTimes[worstNext]=now;worstValues[0][worstNext]=tick;worstValues[1][worstNext]=render;worstValues[2][worstNext]=wasm;worstValues[3][worstNext]=replay;worstValues[4][worstNext]=frameWork;
    worstNext=(worstNext+1)%PERF_ROLLING_CAPACITY;if(worstCount<PERF_ROLLING_CAPACITY)worstCount+=1;
    const cutoff=now-5000;let maxTick=0,maxRender=0,maxWasm=0,maxReplay=0,maxFrame=0;
    for(let sample=0;sample<worstCount;sample+=1)if(worstTimes[sample]>=cutoff){maxTick=Math.max(maxTick,worstValues[0][sample]);maxRender=Math.max(maxRender,worstValues[1][sample]);maxWasm=Math.max(maxWasm,worstValues[2][sample]);maxReplay=Math.max(maxReplay,worstValues[3][sample]);maxFrame=Math.max(maxFrame,worstValues[4][sample]);}
    worstTick=maxTick;worstRender=maxRender;worstWasmRender=maxWasm;worstBrowserReplay=maxReplay;worstFrameWork=maxFrame;
  };
  const colorCache=new Map();
  const cachedColor=(r,g,b)=>{const key=((r&255)<<16)|((g&255)<<8)|(b&255);let value=colorCache.get(key);if(!value){value=color(r,g,b);colorCache.set(key,value);}return value;};
  const DISPLAY_MAX_BACKING_WIDTH=8192,DISPLAY_MAX_BACKING_HEIGHT=8192,DISPLAY_MAX_BACKING_BYTES=64*1024*1024;
  const DISPLAY_TIERS=[1,1.25,1.5,2,3,4,6,8];
  const finitePositive=value=>Number.isFinite(Number(value))&&Number(value)>0?Number(value):0;
  const display={logicalWidth:Math.max(1,Number(canvas.width)||640),logicalHeight:Math.max(1,Number(canvas.height)||360),cssWidth:0,cssHeight:0,backingWidth:Math.max(1,Number(canvas.width)||640),backingHeight:Math.max(1,Number(canvas.height)||360),rawDpr:1,effectiveDpr:1,scaleX:1,scaleY:1,contentScale:1,rasterScale:1,displayGeneration:1,densityGeneration:1,densityTier:1,backingBytes:0,fallback:"none"};
  let resizeGenerationPending=true;
  const setCanvasMetadata=(name,value)=>{if(canvas.dataset)canvas.dataset[name]=String(value);};
  const publishDisplay=()=>{const data=document.body?.dataset;if(data)Object.assign(data,{logicalWidth:String(display.logicalWidth),logicalHeight:String(display.logicalHeight),cssWidth:String(display.cssWidth),cssHeight:String(display.cssHeight),backingWidth:String(display.backingWidth),backingHeight:String(display.backingHeight),backingBytes:String(display.backingBytes),devicePixelRatio:String(display.rawDpr),effectiveDpr:String(display.effectiveDpr),contentScale:String(display.contentScale),rasterScale:String(display.rasterScale),densityTier:String(display.densityTier),displayGeneration:String(display.displayGeneration),densityGeneration:String(display.densityGeneration),backingFallback:display.fallback,backingCap:display.fallback==="none"?"none":"capped"});};
  const syncDisplay=()=>{const bounds=canvas.getBoundingClientRect?.()||{},cssWidth=finitePositive(bounds.width)||finitePositive(canvas.clientWidth)||display.cssWidth||display.logicalWidth,cssHeight=finitePositive(bounds.height)||finitePositive(canvas.clientHeight)||display.cssHeight||display.logicalHeight,requestedDpr=finitePositive(globalThis.devicePixelRatio)||1,rawDpr=Math.max(.5,Math.min(4,requestedDpr)),requestedWidth=Math.max(1,Math.round(cssWidth*rawDpr)),requestedHeight=Math.max(1,Math.round(cssHeight*rawDpr)),bytes=requestedWidth*requestedHeight*4,dimensions=Math.min(1,DISPLAY_MAX_BACKING_WIDTH/requestedWidth,DISPLAY_MAX_BACKING_HEIGHT/requestedHeight),byteScale=bytes>DISPLAY_MAX_BACKING_BYTES?Math.sqrt(DISPLAY_MAX_BACKING_BYTES/bytes):1,capScale=Math.min(1,dimensions,byteScale),effectiveDpr=rawDpr*capScale;let backingWidth=Math.max(1,Math.min(DISPLAY_MAX_BACKING_WIDTH,Math.round(cssWidth*effectiveDpr))),backingHeight=Math.max(1,Math.min(DISPLAY_MAX_BACKING_HEIGHT,Math.round(cssHeight*effectiveDpr)));while(backingWidth*backingHeight*4>DISPLAY_MAX_BACKING_BYTES){const correction=Math.sqrt(DISPLAY_MAX_BACKING_BYTES/(backingWidth*backingHeight*4)),nextWidth=Math.max(1,Math.floor(backingWidth*correction)),nextHeight=Math.max(1,Math.floor(backingHeight*correction));if(nextWidth===backingWidth&&nextHeight===backingHeight){if(backingWidth>=backingHeight)backingWidth-=1;else backingHeight-=1;}else{backingWidth=nextWidth;backingHeight=nextHeight;}}const scaleX=backingWidth/Math.max(1,display.logicalWidth),scaleY=backingHeight/Math.max(1,display.logicalHeight),contentScale=Math.min(scaleX,scaleY),rasterScale=Math.max(1,Math.min(8,contentScale)),densityTier=DISPLAY_TIERS.find(tier=>tier+1e-9>=rasterScale)||8,extentChanged=display.cssWidth!==cssWidth||display.cssHeight!==cssHeight||display.backingWidth!==backingWidth||display.backingHeight!==backingHeight,rasterScaleChanged=display.rasterScale!==rasterScale;Object.assign(display,{cssWidth,cssHeight,rawDpr,effectiveDpr,backingWidth,backingHeight,scaleX,scaleY,contentScale,rasterScale,densityTier,backingBytes:backingWidth*backingHeight*4,fallback:[requestedDpr!==rawDpr?"dpr":"",dimensions<1?"dimension":"",byteScale<1?"bytes":""].filter(Boolean).join(",")||"none"});if(extentChanged&&!resizeGenerationPending){display.displayGeneration+=1;resizeGenerationPending=true;}if(rasterScaleChanged)display.densityGeneration+=1;if(canvas.width!==backingWidth)canvas.width=backingWidth;if(canvas.height!==backingHeight)canvas.height=backingHeight;setCanvasMetadata("logicalWidth",display.logicalWidth);setCanvasMetadata("logicalHeight",display.logicalHeight);publishDisplay();resizeGenerationPending=false;};
  setCanvasMetadata("logicalWidth",display.logicalWidth);setCanvasMetadata("logicalHeight",display.logicalHeight);
  window.STASIS_REFIT_VIEWPORT?.();

  function executeCommands() {
    syncDisplay();
    context.setTransform?.(display.scaleX,0,0,display.scaleY,0,0);
    for (const command of commands) {
      if (command[0] === 0) {
        context.globalAlpha=1; context.fillStyle = cachedColor(command[1], command[2], command[3]);
        context.fillRect(0, 0, display.logicalWidth, display.logicalHeight);
      } else if (command[0] === 1) {
        context.globalAlpha=1; context.fillStyle = cachedColor(command[5], command[6], command[7]);
        context.fillRect(command[1], command[2], command[3], command[4]);
      }
      else if (command[0] === 2) {
        context.fillStyle = "#dff6ff";
        context.font = "18px ui-monospace, Consolas, monospace";
        context.fillText(`score ${command[3]}`, command[1], command[2]);
      }
    }
  }

  function frame() {
    const tickStart = performance.now();
    instance.exports.tick();
    const tickMs = performance.now() - tickStart;
    const wasmRenderStart=performance.now();
    instance.exports.render();
    const wasmRenderMs=performance.now()-wasmRenderStart;
    const replayStart=performance.now();
    executeCommands();
    const browserReplayMs=performance.now()-replayStart;
    const renderMs=wasmRenderMs+browserReplayMs;
    const frameWorkMs=tickMs+renderMs;
    const renderPrepMs=-1,gpuSubmitMs=-1,gpuExecutionMs=-1,presentWaitMs=-1;
    frames += 1;
    if (hud) {
      recordWorst(performance.now(),tickMs,renderMs,wasmRenderMs,browserReplayMs,frameWorkMs);
    }
    const underBudget=frameWorkMs<=16.67;
    let lines=0,rectangles=0,text=0;
    for(const command of commands){if(command[0]===1)rectangles+=1;else if(command[0]===2)text+=1;}
    if (hud) hud.textContent=`Canvas2D · frame ${frames}\ntick ${tickMs.toFixed(3)} ms (worst ${worstTick.toFixed(3)}) · guest render ${wasmRenderMs.toFixed(3)} ms (worst ${worstWasmRender.toFixed(3)})\nhost replay ${browserReplayMs.toFixed(3)} ms (worst ${worstBrowserReplay.toFixed(3)})\nframe work ${frameWorkMs.toFixed(3)} ms (worst ${worstFrameWork.toFixed(3)}) · ${underBudget?"UNDER 16.67 ms":"OVER 16.67 ms"}\ncommands ${commands.length} · lines ${lines} · rects ${rectangles} · text ${text}\ndraws ${commands.length}`;
    document.body.dataset.frames = String(frames);
    document.body.dataset.tickMs=tickMs.toFixed(3);document.body.dataset.renderMs=renderMs.toFixed(3);document.body.dataset.wasmRenderMs=wasmRenderMs.toFixed(3);document.body.dataset.browserReplayMs=browserReplayMs.toFixed(3);document.body.dataset.frameWorkMs=frameWorkMs.toFixed(3);document.body.dataset.worstTickMs=worstTick.toFixed(3);document.body.dataset.worstRenderMs=worstRender.toFixed(3);document.body.dataset.worstWasmRenderMs=worstWasmRender.toFixed(3);document.body.dataset.worstBrowserReplayMs=worstBrowserReplay.toFixed(3);document.body.dataset.worstFrameWorkMs=worstFrameWork.toFixed(3);
    document.body.dataset.underBudget = String(underBudget);
    document.body.dataset.backend="Canvas2D";
    document.body.dataset.hostReplayMs=browserReplayMs.toFixed(3);
    document.body.dataset.renderPrepMs=String(renderPrepMs);
    document.body.dataset.gpuSubmitMs=String(gpuSubmitMs);
    document.body.dataset.gpuExecutionMs=String(gpuExecutionMs);
    document.body.dataset.presentWaitMs=String(presentWaitMs);
    document.body.dataset.commands=String(commands.length);
    document.body.dataset.lines=String(lines);
    document.body.dataset.rectangles=String(rectangles);
    document.body.dataset.sprites="-1";
    document.body.dataset.text=String(text);
    document.body.dataset.instances="-1";
    document.body.dataset.batches="-1";
    document.body.dataset.drawCalls=String(commands.length);
    document.body.dataset.uploadedBytes="-1";
    requestAnimationFrame(frame);
  }

  (async () => {
    try {
      setLoading("Preparing…", "loading");
      const response = await fetch("game.wasm");
      if (!response.ok) throw new Error(`failed to load game.wasm: ${response.status}`);
      instance = (await WebAssembly.instantiate(await response.arrayBuffer(), imports)).instance;
      document.body.dataset.mainResult = String(instance.exports.main());
      document.body.dataset.ready = "true";
      setLoading("", "ready");
      document.body.dataset.runtime = "wasm";
      requestAnimationFrame(frame);
    } catch (error) {
      document.body.dataset.ready = "false";
      setLoading(`Unable to start this game. ${String(error && error.message || error)}`, "failed");
      errorBox.textContent = String(error && error.stack || error);
      throw error;
    }
  })();
})();
