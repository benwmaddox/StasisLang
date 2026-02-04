namespace Stasis.Compiler.IR.Bytecode;

public enum BytecodeOp : byte
{
    Nop = 0,

    Pop = 2,
    Dup = 3,

    // Stack constants
    ConstI32 = 1, // a = i32
    ConstF32 = 4, // a = f32 bits (BitConverter.SingleToInt32Bits)

    // Locals
    LoadLocalI32 = 10,  // a = local index
    StoreLocalI32 = 11, // a = local index
    LoadLocalF32 = 12,  // a = local index
    StoreLocalF32 = 13, // a = local index

    // Globals
    LoadGlobalI32 = 20,  // a = global index
    StoreGlobalI32 = 21, // a = global index
    LoadGlobalF32 = 22,  // a = global index
    StoreGlobalF32 = 23, // a = global index

    // Arithmetic
    AddI32 = 30,
    SubI32 = 31,
    MulI32 = 32,
    DivI32 = 33,
    NegI32 = 34,

    AddF32 = 35,
    SubF32 = 36,
    MulF32 = 37,
    DivF32 = 38,
    NegF32 = 39,

    // Comparisons (push i32 0/1)
    CmpEqI32 = 70,
    CmpNeI32 = 71,
    CmpLtI32 = 72,
    CmpLeI32 = 73,
    CmpGtI32 = 74,
    CmpGeI32 = 75,
    CmpEqF32 = 76,
    CmpNeF32 = 77,
    CmpLtF32 = 78,
    CmpLeF32 = 79,
    CmpGtF32 = 80,
    CmpGeF32 = 81,

    NotI32 = 82,

    // Control flow
    Jump = 40,        // a = absolute instruction index
    JumpIfZeroI32 = 41, // pops i32; if == 0, jump to a

    // Calls
    Call = 45, // a = function index, b = arg count

    // Returns
    ReturnI32 = 50, // pops i32 and returns it
    ReturnF32 = 52, // pops f32 bits and returns it (as i32 bits)
    ReturnVoid = 51
}
