set(STASIS_THORVG_SOURCES
    src/common/tvgCompressor.cpp
    src/common/tvgMath.cpp
    src/common/tvgStr.cpp
    src/renderer/tvgAccessor.cpp
    src/renderer/tvgAnimation.cpp
    src/renderer/tvgCanvas.cpp
    src/renderer/tvgFill.cpp
    src/renderer/tvgInitializer.cpp
    src/renderer/tvgLoaderMgr.cpp
    src/renderer/tvgPaint.cpp
    src/renderer/tvgPicture.cpp
    src/renderer/tvgRender.cpp
    src/renderer/tvgSaver.cpp
    src/renderer/tvgScene.cpp
    src/renderer/tvgShape.cpp
    src/renderer/tvgTaskScheduler.cpp
    src/renderer/tvgText.cpp
    src/renderer/cpu_engine/tvgSwBlendOp.cpp
    src/renderer/cpu_engine/tvgSwFill.cpp
    src/renderer/cpu_engine/tvgSwImage.cpp
    src/renderer/cpu_engine/tvgSwMemPool.cpp
    src/renderer/cpu_engine/tvgSwPostEffect.cpp
    src/renderer/cpu_engine/tvgSwRaster.cpp
    src/renderer/cpu_engine/tvgSwRenderer.cpp
    src/renderer/cpu_engine/tvgSwRle.cpp
    src/renderer/cpu_engine/tvgSwShape.cpp
    src/renderer/cpu_engine/tvgSwStroke.cpp
    src/renderer/cpu_engine/tvgSwUtil.cpp
    src/loaders/raw/tvgRawLoader.cpp
    src/loaders/svg/tvgSvgBuilder.cpp
    src/loaders/svg/tvgSvgCssStyle.cpp
    src/loaders/svg/tvgSvgLoader.cpp
    src/loaders/svg/tvgSvgPath.cpp
    src/loaders/svg/tvgSvgUtil.cpp
    src/loaders/svg/tvgXmlParser.cpp
    src/bindings/capi/tvgCapi.cpp
)

list(TRANSFORM STASIS_THORVG_SOURCES PREPEND "${CMAKE_CURRENT_LIST_DIR}/")

add_library(stasis_thorvg STATIC ${STASIS_THORVG_SOURCES})
target_compile_features(stasis_thorvg PUBLIC cxx_std_14)
target_compile_definitions(stasis_thorvg PUBLIC TVG_STATIC PRIVATE NOMINMAX)
target_include_directories(stasis_thorvg
    PUBLIC "${CMAKE_CURRENT_LIST_DIR}/src/bindings/capi"
    PRIVATE
        "${CMAKE_CURRENT_LIST_DIR}"
        "${CMAKE_CURRENT_LIST_DIR}/inc"
        "${CMAKE_CURRENT_LIST_DIR}/src"
        "${CMAKE_CURRENT_LIST_DIR}/src/common"
        "${CMAKE_CURRENT_LIST_DIR}/src/renderer"
        "${CMAKE_CURRENT_LIST_DIR}/src/renderer/cpu_engine"
        "${CMAKE_CURRENT_LIST_DIR}/src/loaders"
        "${CMAKE_CURRENT_LIST_DIR}/src/loaders/raw"
        "${CMAKE_CURRENT_LIST_DIR}/src/loaders/svg"
)
set_target_properties(stasis_thorvg PROPERTIES POSITION_INDEPENDENT_CODE ON)

if(MSVC)
    target_compile_options(stasis_thorvg PRIVATE /EHsc /wd4244 /wd4267)
else()
    target_compile_options(stasis_thorvg PRIVATE
        -fno-exceptions
        -fno-rtti
        -Wno-unknown-pragmas
        -Wno-double-promotion
    )
endif()

if(NOT WIN32 AND NOT ANDROID AND NOT EMSCRIPTEN)
    find_package(Threads REQUIRED)
    target_link_libraries(stasis_thorvg PUBLIC Threads::Threads)
endif()
