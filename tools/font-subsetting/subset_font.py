"""Embedded driver used by Stasis release font subsetting."""

import json
import re
import sys


EXPECTED_FONTTOOLS_VERSION = "4.60.2"
DRIVER_VERSION = 1
FAMILY_NAME_IDS = (1, 2, 3, 4, 6, 16, 17, 21, 22)
REQUIRED_NAME_IDS = (1, 2, 3, 4, 6)
LICENSE_NAME_IDS = (13, 14)
PRESERVED_TABLES = (
    "BASE",
    "CFF ",
    "CFF2",
    "GDEF",
    "GPOS",
    "GSUB",
    "JSTF",
    "OS/2",
    "VORG",
    "cvt ",
    "fpgm",
    "gasp",
    "glyf",
    "hmtx",
    "maxp",
    "prep",
    "vmtx",
)


def result(**fields):
    fields["driver_version"] = DRIVER_VERSION
    print(json.dumps(fields, sort_keys=True, separators=(",", ":")))


def normalized_name(value):
    return re.sub(r"\s+", " ", value).strip().casefold()


def contains_reserved_name(value, reserved_names):
    candidate = re.sub(r"[^0-9a-z]+", "", normalized_name(value))
    for reserved in reserved_names:
        name = re.sub(r"[^0-9a-z]+", "", normalized_name(reserved))
        if name and name in candidate:
            return True
    return False


def decoded_name_records(font):
    if "name" not in font:
        return None, "source_name_table_missing"
    decoded = []
    for record in font["name"].names:
        try:
            value = record.toUnicode().strip()
        except Exception:
            return None, "source_name_metadata_undecodable"
        if value:
            decoded.append((record.nameID, value))
    for name_id in REQUIRED_NAME_IDS:
        if not any(record_id == name_id for record_id, _ in decoded):
            return None, "source_name_metadata_missing"
    if not any(record_id in LICENSE_NAME_IDS for record_id, _ in decoded):
        return None, "source_license_metadata_missing"
    return decoded, None


def quoted_reserved_names(value):
    matches = re.findall(
        r"reserved\s+font\s+names?\s*(?:is|are|:|-)?\s*[\"']([^\"']+)[\"']",
        value,
        flags=re.IGNORECASE,
    )
    return [match.strip() for match in matches if match.strip()]


def validate_source_metadata(font, reserved_names):
    decoded, reason = decoded_name_records(font)
    if reason is not None:
        return reason
    assert decoded is not None
    declarations = [
        value for _, value in decoded if "reserved font name" in value.casefold()
    ]
    if declarations and not reserved_names:
        return "reserved_name_metadata_ambiguous"
    for declared in declarations:
        for detected in quoted_reserved_names(declared):
            if not any(
                normalized_name(detected) == normalized_name(configured)
                for configured in reserved_names
            ):
                return "undeclared_reserved_name"
    for reserved in reserved_names:
        if not any(contains_reserved_name(value, [reserved]) for _, value in decoded):
            return "reserved_name_not_in_source_metadata"
    return None


def subset_options(subset_module):
    options = subset_module.Options()
    options.canonical_order = True
    options.glyph_names = True
    options.hinting = True
    options.layout_features = ["*"]
    options.layout_scripts = ["*"]
    options.legacy_cmap = True
    options.name_IDs = ["*"]
    options.name_languages = ["*"]
    options.notdef_glyph = True
    options.notdef_outline = True
    options.recommended_glyphs = True
    options.preserve_gids = False
    return options


def rename_family(font, replacement_family):
    if replacement_family is None:
        return
    name_table = font["name"]
    subfamily = name_table.getDebugName(2)
    if not subfamily:
        raise ValueError("source subfamily name is missing")
    postscript_family = re.sub(r"[^A-Za-z0-9-]+", "-", replacement_family).strip("-")
    if not postscript_family:
        raise ValueError("replacement family cannot form a PostScript name")
    replacements = {
        1: replacement_family,
        3: replacement_family + "; " + subfamily + "; Stasis subset",
        2: subfamily,
        4: replacement_family + " " + subfamily,
        6: postscript_family + "-" + re.sub(r"[^A-Za-z0-9-]+", "-", subfamily).strip("-"),
        16: replacement_family,
        17: subfamily,
        21: replacement_family,
        22: subfamily,
    }
    records = list(name_table.names)
    for record in records:
        replacement = replacements.get(record.nameID)
        if replacement is not None:
            name_table.setName(
                replacement,
                record.nameID,
                record.platformID,
                record.platEncID,
                record.langID,
            )


def table_set(font):
    return set(font.keys())


def glyph_drawing(font, glyph_name):
    from fontTools.pens.recordingPen import RecordingPen

    pen = RecordingPen()
    font.getGlyphSet()[glyph_name].draw(pen)
    return pen.value


def validate_subset(source, output, scalars, reserved_names, replacement_family):
    source_metadata_reason = validate_source_metadata(source, reserved_names)
    if source_metadata_reason is not None:
        return source_metadata_reason, []

    output_cmap = output.getBestCmap() or {}
    missing = sorted(scalar for scalar in scalars if scalar not in output_cmap)
    if missing:
        return "missing_glyph", missing

    if ".notdef" not in output.getGlyphOrder():
        return "notdef_missing", []
    if "glyf" in output and not output["glyf"][".notdef"].numberOfContours and not output["glyf"][".notdef"].components:
        if ".notdef" in source.getGlyphOrder() and source["glyf"][".notdef"].numberOfContours:
            return "notdef_outline_missing", []

    source_tables = table_set(source)
    output_tables = table_set(output)
    missing_tables = sorted(table for table in PRESERVED_TABLES if table in source_tables and table not in output_tables)
    if missing_tables:
        return "preserved_table_missing", []

    for table_name in ("hmtx", "vmtx"):
        if table_name not in source_tables or table_name not in output_tables:
            continue
        original_metrics = source[table_name].metrics
        output_metrics = output[table_name].metrics
        for glyph_name, metrics in output_metrics.items():
            if glyph_name in original_metrics and original_metrics[glyph_name] != metrics:
                return "metrics_changed", []

    source_glyphs = set(source.getGlyphOrder())
    for glyph_name in output.getGlyphOrder():
        if glyph_name in source_glyphs and glyph_drawing(source, glyph_name) != glyph_drawing(output, glyph_name):
            return "outline_changed", []

    if "glyf" in output:
        output_glyphs = output["glyf"]
        for glyph in output_glyphs.glyphs.values():
            if glyph.isComposite():
                for component in glyph.components:
                    if component.glyphName not in output_glyphs:
                        return "composite_component_missing", []

    _output_names, output_name_reason = decoded_name_records(output)
    if output_name_reason is not None:
        return "output_name_metadata_invalid", []
    if "name" in output:
        if replacement_family is not None:
            family_names = [
                output["name"].getDebugName(name_id)
                for name_id in (1, 16)
                if output["name"].getDebugName(name_id)
            ]
            if not family_names or any(name != replacement_family for name in family_names):
                return "replacement_family_mismatch", []
        for name_id in FAMILY_NAME_IDS:
            for record in output["name"].names:
                if record.nameID == name_id and contains_reserved_name(record.toUnicode(), reserved_names):
                    return "reserved_name_remains", []
    return None, []


def main():
    try:
        import fontTools
        from fontTools import subset as subset_module
        from fontTools.ttLib import TTFont
    except Exception as error:
        result(ok=False, reason="tool_unavailable", detail=str(error))
        return 2

    if getattr(fontTools, "__version__", None) != EXPECTED_FONTTOOLS_VERSION:
        result(
            ok=False,
            reason="exact_tool_unavailable",
            detail="expected fonttools==" + EXPECTED_FONTTOOLS_VERSION,
            found_version=getattr(fontTools, "__version__", None),
        )
        return 2

    try:
        request = json.load(sys.stdin)
        action = request["action"]
        source = TTFont(request["input"], recalcBBoxes=False, recalcTimestamp=False)
        scalars = sorted(set(int(value) for value in request["scalars"]))
        reserved_names = request.get("reserved_names", [])
        replacement_family = request.get("replacement_family")
        source_metadata_reason = validate_source_metadata(source, reserved_names)
        if source_metadata_reason is not None:
            result(ok=False, reason=source_metadata_reason)
            return 4
        source_cmap = source.getBestCmap() or {}
        missing = sorted(scalar for scalar in scalars if scalar not in source_cmap)
        if missing:
            result(ok=False, reason="missing_glyph", missing_codepoints=missing)
            return 3

        if action == "subset":
            original = TTFont(request["input"], recalcBBoxes=False, recalcTimestamp=False)
            if reserved_names and replacement_family is None:
                result(ok=False, reason="reserved_name_replacement_required")
                return 4
            if replacement_family is not None and contains_reserved_name(replacement_family, reserved_names):
                result(ok=False, reason="replacement_family_is_reserved")
                return 4

            subsetter = subset_module.Subsetter(options=subset_options(subset_module))
            subsetter.populate(unicodes=scalars)
            subsetter.subset(source)
            rename_family(source, replacement_family)
            source.recalcTimestamp = False
            source.save(request["output"], reorderTables=True)
            output = TTFont(request["output"], recalcBBoxes=False, recalcTimestamp=False)
            reason, missing_after = validate_subset(
                original, output, scalars, reserved_names, replacement_family
            )
        elif action == "validate":
            output = source
            original = TTFont(request["original"], recalcBBoxes=False, recalcTimestamp=False)
            reason, missing_after = validate_subset(
                original, output, scalars, reserved_names, replacement_family
            )
        else:
            result(ok=False, reason="invalid_action")
            return 2

        if reason is not None:
            result(ok=False, reason=reason, missing_codepoints=missing_after)
            return 5
        result(ok=True)
        return 0
    except Exception as error:
        result(ok=False, reason="transform_error", detail=str(error))
        return 6


if __name__ == "__main__":
    sys.exit(main())
