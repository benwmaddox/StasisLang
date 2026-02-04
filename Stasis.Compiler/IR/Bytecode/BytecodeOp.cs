namespace Stasis.Compiler.IR.Bytecode;

public enum BytecodeOp : byte
{
    Nop = 0,

    // Stack constants
    ConstI32 = 1, // a = i32

    // Locals
    LoadLocalI32 = 10,  // a = local index
    StoreLocalI32 = 11, // a = local index

    // Globals
    LoadGlobalI32 = 20,  // a = global index
    StoreGlobalI32 = 21, // a = global index

    // Arithmetic
    AddI32 = 30,
    SubI32 = 31,
    MulI32 = 32,
    DivI32 = 33,

    // Control flow
    Jump = 40,        // a = absolute instruction index
    JumpIfZeroI32 = 41, // pops i32; if == 0, jump to a

    // Returns
    ReturnI32 = 50, // pops i32 and returns it
    ReturnVoid = 51
}

