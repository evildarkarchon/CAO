# Cathedral Assets Optimizer

Language for CAO's Bethesda game asset optimization workflow.

## Language

**Asset**:
A file in a mod that CAO can inspect or transform, such as a texture, mesh, animation, or BSA archive.
_Avoid_: Resource

**Mod**:
A directory of Bethesda game assets selected for CAO to optimize.
_Avoid_: Folder

**Ignored Mod**:
A mod intentionally excluded from an Asset Work Plan when CAO is processing several mods.
_Avoid_: Excluded folder, skipped directory

**Profile**:
The selected Bethesda game configuration that determines which asset kinds and archive rules CAO applies.
_Avoid_: Preset

**Asset Work Options**:
The validated user selections that request asset work before CAO resolves them against the selected Profile. They include work mode and Dry Run, but not the selected path or logging preferences; raw UI, CLI, and settings input is not yet Asset Work Options.
_Avoid_: Settings, UI state, OptionsCAO

**Asset Work Plan**:
The ordered description of asset work CAO intends for a selected input path, including selected mods, archive extraction, loose asset discovery, loose asset processing, and archive packing. It is a plan, not execution.
_Avoid_: Processing pipeline, optimization run

**Asset Work Policy**:
The resolved rules that decide which kinds of asset work are allowed for a selected Profile and requested options before CAO creates an Asset Work Plan. It is policy, not the ordered work itself.
_Avoid_: Profile snapshot, settings policy, option flags

**Asset Work Execution Policy**:
The resolved rules and parameters used while carrying out an Asset Work Plan for a selected Profile and requested options. It is execution policy, not the decision of which Asset Work Items exist.
_Avoid_: Optimizer settings, runtime options, execution config

**Asset Work Plan Execution**:
The act of carrying out an Asset Work Plan against the selected Mod or Mods. It is execution, not planning; Loose Asset Discovery may happen during it after archive extraction changes the available Assets.
_Avoid_: Optimization run, processing pipeline, manager orchestration

**Dry Run**:
An Asset Work Plan Execution that reports intended Asset work without changing Assets. Archive extraction and packing remain intended work, but Loose Asset Discovery cannot include Assets that would only become available through extraction.
_Avoid_: Preview mode, no-op plan

**Loose Asset Discovery**:
The pass that classifies loose assets after archive extraction has had a chance to add files to the selected input path.
_Avoid_: File scan

**Asset Work Item**:
One classified piece of intended asset work inside an Asset Work Plan. It identifies what asset or folder is involved, not the optimization policy used to perform the work.
_Avoid_: Task, job

**Mod Asset Metadata**:
Facts CAO derives from selected Mod contents and Profile-provided reference lists to interpret Assets during Asset Work Plan Execution, such as whether a mesh is a headpart.
_Avoid_: Preparation data, plugin scan results

**Headpart**:
A mesh Asset used for character head or face parts that CAO treats as a special mesh case during Asset Work Plan Execution.
_Avoid_: Headpart path, headpart file
