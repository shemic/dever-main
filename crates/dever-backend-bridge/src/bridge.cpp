// Private LLVM 18 / LLD bridge. Inputs are borrowed only for the call; outputs
// use malloc/free through one ABI and never expose LLVM ownership to Rust.
#include "lld/Common/Driver.h"
#include "lld/Common/ErrorHandler.h"
#include "llvm/IR/LegacyPassManager.h"
#include "llvm/IR/Constants.h"
#include "llvm/IR/Verifier.h"
#include "llvm/IRReader/IRReader.h"
#include "llvm/MC/TargetRegistry.h"
#include "llvm/Passes/PassBuilder.h"
#include "llvm/Support/SourceMgr.h"
#include "llvm/Support/TargetSelect.h"
#include "llvm/Target/TargetMachine.h"
#include <cstdlib>
#include <cstring>
#include <mutex>

LLD_HAS_DRIVER(elf)
LLD_HAS_DRIVER(coff)
LLD_HAS_DRIVER(macho)

struct Reply {
  unsigned char *bytes;
  size_t len;
  uint32_t status;
};

struct Resource {
  const unsigned char *symbol;
  size_t symbol_len;
  const unsigned char *bytes;
  size_t len;
};

static void answer(Reply *reply, llvm::StringRef bytes, uint32_t status) {
  constexpr size_t limit = 320 * 1024 * 1024;
  if (bytes.size() > limit) {
    bytes = "compiler bridge output exceeds 320 MiB";
    status = 1;
  }
  reply->status = status;
  reply->len = bytes.size();
  reply->bytes = static_cast<unsigned char *>(std::malloc(bytes.size()));
  if (!reply->bytes && !bytes.empty()) {
    reply->len = 0;
    reply->status = 1;
    return;
  }
  if (!bytes.empty()) std::memcpy(reply->bytes, bytes.data(), bytes.size());
}

extern "C" void dever_backend_free(unsigned char *bytes) { std::free(bytes); }

extern "C" void dever_backend_object(const unsigned char *ir, size_t len,
                                     const char *triple, const Resource *resources,
                                     size_t resource_count, Reply *reply) {
  try {
    static std::once_flag initialized;
    std::call_once(initialized, [] {
      llvm::InitializeAllTargetInfos();
      llvm::InitializeAllTargets();
      llvm::InitializeAllTargetMCs();
      llvm::InitializeAllAsmPrinters();
      llvm::InitializeAllAsmParsers();
    });
    llvm::LLVMContext context;
    llvm::SMDiagnostic diagnostic;
    auto input = llvm::MemoryBuffer::getMemBufferCopy(
        llvm::StringRef(reinterpret_cast<const char *>(ir), len), "dever.ir");
    auto module = llvm::parseIR(input->getMemBufferRef(), diagnostic, context);
    std::string error;
    llvm::raw_string_ostream errors(error);
    if (!module) {
      diagnostic.print("dever", errors);
      return answer(reply, error, 1);
    }
    // Do not silently compile a module for a target different from its ABI.
    if (!module->getTargetTriple().empty() && module->getTargetTriple() != triple)
      return answer(reply, "LLVM module target disagrees with selected target", 1);
    for (size_t index = 0; index < resource_count; ++index) {
      const auto &resource = resources[index];
      auto *global = module->getNamedGlobal(llvm::StringRef(
          reinterpret_cast<const char *>(resource.symbol), resource.symbol_len));
      auto *array = global ? llvm::dyn_cast<llvm::ArrayType>(global->getValueType()) : nullptr;
      if (!global || !global->isDeclaration() || !global->isConstant() ||
          !global->hasExternalLinkage() || global->isThreadLocal() ||
          global->getAddressSpace() != 0 || !array ||
          !array->getElementType()->isIntegerTy(8) ||
          array->getNumElements() != resource.len || global->use_empty())
        return answer(reply, "LLVM resource must match a used external constant byte-array declaration", 1);
      global->setInitializer(llvm::ConstantDataArray::get(
          context, llvm::ArrayRef<uint8_t>(resource.bytes, resource.len)));
      global->setLinkage(llvm::GlobalValue::PrivateLinkage);
    }
    if (llvm::verifyModule(*module, &errors)) return answer(reply, error, 1);
    auto target = llvm::TargetRegistry::lookupTarget(triple, error);
    if (!target) return answer(reply, error, 1);
    llvm::TargetOptions options;
    std::unique_ptr<llvm::TargetMachine> machine(target->createTargetMachine(
        triple, "generic", "", options, llvm::Reloc::PIC_, std::nullopt,
        llvm::CodeGenOptLevel::Default));
    if (!machine) return answer(reply, "LLVM target machine is unavailable", 1);
    auto layout = machine->createDataLayout();
    if (!module->getDataLayoutStr().empty() && module->getDataLayout() != layout)
      return answer(reply, "LLVM module layout disagrees with selected target", 1);
    module->setTargetTriple(triple);
    module->setDataLayout(layout);
    llvm::PassBuilder passes(machine.get());
    llvm::LoopAnalysisManager loops;
    llvm::FunctionAnalysisManager functions;
    llvm::CGSCCAnalysisManager calls;
    llvm::ModuleAnalysisManager modules;
    passes.registerModuleAnalyses(modules);
    passes.registerCGSCCAnalyses(calls);
    passes.registerFunctionAnalyses(functions);
    passes.registerLoopAnalyses(loops);
    passes.crossRegisterProxies(loops, functions, calls, modules);
    auto pipeline = passes.buildPerModuleDefaultPipeline(llvm::OptimizationLevel::O2);
    pipeline.run(*module, modules);
    if (llvm::verifyModule(*module, &errors)) return answer(reply, error, 1);
    llvm::SmallVector<char, 0> output;
    llvm::raw_svector_ostream stream(output);
    llvm::legacy::PassManager emission;
    if (machine->addPassesToEmitFile(emission, stream, nullptr,
                                   llvm::CodeGenFileType::ObjectFile))
      return answer(reply, "LLVM target cannot emit native objects", 1);
    emission.run(*module);
    answer(reply, llvm::StringRef(output.data(), output.size()), 0);
  } catch (const std::exception &error) {
    answer(reply, error.what(), 1);
  } catch (...) {
    answer(reply, "LLVM bridge failed", 1);
  }
}

extern "C" void dever_backend_link(const char *const *args, size_t count,
                                   Reply *reply) {
  try {
    std::string output, error;
    llvm::raw_string_ostream out(output), err(error);
    const lld::DriverDef drivers[] = {{lld::Gnu, lld::elf::link},
                                     {lld::WinLink, lld::coff::link},
                                     {lld::Darwin, lld::macho::link}};
    auto result = lld::lldMain(llvm::ArrayRef(args, count), out, err, drivers);
    // LLD explicitly forbids cleanup/re-entry after failed crash recovery.
    // This bridge belongs to a compiler worker, never an application process.
    if (!result.canRunAgain) lld::exitLld(result.retCode ? result.retCode : 1);
    answer(reply, error, result.retCode == 0 ? 0 : 1);
  } catch (const std::exception &) {
    lld::exitLld(1);
  } catch (...) {
    lld::exitLld(1);
  }
}
