/* tslint:disable */
/* eslint-disable */

export function analyze_snapshot(snapshot: any, entrypoint: string, source_override?: string | null): any;

export function apply_changes(changes: any): any;

export function changes_between(base: any, candidate: any): any;

export function complete_current(entrypoint: string, source: string, utf16_offset: number, explicit: boolean): any;

export function compose_schema_bundle(contributions: any): any;

export function evaluate_current(contract: any): any;

export function evaluate_snapshot(snapshot: any, contract: any): any;

export function formatSource(source: string): string;

export function format_source(source: string): string;

export function set_schema_bundle(schema_bundle: any): void;

export function set_snapshot(snapshot: any): void;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly analyze_snapshot: (a: any, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly apply_changes: (a: any) => [number, number, number];
    readonly changes_between: (a: any, b: any) => [number, number, number];
    readonly complete_current: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly compose_schema_bundle: (a: any) => [number, number, number];
    readonly evaluate_current: (a: any) => [number, number, number];
    readonly evaluate_snapshot: (a: any, b: any) => [number, number, number];
    readonly format_source: (a: number, b: number) => [number, number, number, number];
    readonly set_schema_bundle: (a: any) => [number, number];
    readonly set_snapshot: (a: any) => [number, number];
    readonly formatSource: (a: number, b: number) => [number, number];
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
