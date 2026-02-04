namespace Stasis.Compiler.IR.Bytecode;

public readonly record struct BytecodeInst(BytecodeOp Op, int A = 0, int B = 0);
