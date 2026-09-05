# 01. Three entry points

What are the smallest roles in a Stasis game?

The Stasis host recognizes `main`, `tick`, and `render` as lifecycle roots.
The project decides what each boundary delegates to. Here, `main` establishes
state, `tick` advances one logical transition, and `render` builds
presentation data.

```stasis
function main(): i32 {
    reset_simulation();
    configure_example_wave();
    return 0;
}
```

Later nuggets define the state owners, show the recipe directly inside `tick`,
and build the projection inside `render`.

**Keep:** Stasis owns the lifecycle boundary; project code gives each root one
clear responsibility.
