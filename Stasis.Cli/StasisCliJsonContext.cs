using System.Collections.Generic;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Stasis.Cli;

[JsonSerializable(typeof(TestCacheEntry))]
[JsonSerializable(typeof(StructMetadata))]
internal partial class StasisCliJsonContext : JsonSerializerContext
{
}

internal static class StasisCliJson
{
    internal static readonly StasisCliJsonContext Default = new(new JsonSerializerOptions(JsonSerializerDefaults.General));
    internal static readonly StasisCliJsonContext Indented = new(new JsonSerializerOptions(JsonSerializerDefaults.General)
    {
        WriteIndented = true
    });
}

internal sealed class StructFieldMetadata
{
    [JsonPropertyName("name")]
    public string Name { get; init; } = string.Empty;

    [JsonPropertyName("jsonPath")]
    public string JsonPath { get; init; } = string.Empty;

    [JsonPropertyName("offset")]
    public int Offset { get; init; }

    [JsonPropertyName("size")]
    public int Size { get; init; }

    [JsonPropertyName("type")]
    public string Type { get; init; } = string.Empty;

    [JsonPropertyName("arrayCount")]
    public int ArrayCount { get; init; }
}

internal sealed class StructMetadata
{
    [JsonPropertyName("version")]
    public int Version { get; init; }

    [JsonPropertyName("globalName")]
    public string GlobalName { get; init; } = string.Empty;

    [JsonPropertyName("totalSize")]
    public int TotalSize { get; init; }

    [JsonPropertyName("fields")]
    public List<StructFieldMetadata> Fields { get; init; } = new();
}
