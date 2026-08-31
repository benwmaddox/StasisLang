if(NOT EXISTS "${DEF_FILE}")
    message(FATAL_ERROR "missing graphics export definition: ${DEF_FILE}")
endif()

file(STRINGS "${DEF_FILE}" graphics_exports)
foreach(required_export IN ITEMS
    stasis_gfx_set_next_sprite_atlas_policy_v3
    stasis_asset_request_sprite_with_policy_v3
)
    list(FIND graphics_exports "    ${required_export}" export_index)
    if(export_index EQUAL -1)
        message(FATAL_ERROR "missing v3 sprite atlas export: ${required_export}")
    endif()
endforeach()
