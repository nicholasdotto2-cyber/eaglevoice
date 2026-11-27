// Dichiarazione minima di `bun:test` per `tsc`: il progetto non ha `bun-types`
// fra le dipendenze e i test girano con `bun test`, che fornisce il modulo vero.
declare module "bun:test" {
  type Matchers = {
    toBe(expected: unknown): void;
    toEqual(expected: unknown): void;
    toBeNull(): void;
    toHaveLength(length: number): void;
    toContain(item: unknown): void;
    toBeGreaterThan(n: number): void;
    toBeGreaterThanOrEqual(n: number): void;
    toBeLessThanOrEqual(n: number): void;
    toMatchObject(expected: object): void;
    not: Matchers;
  };
  export function test(name: string, fn: () => void | Promise<void>): void;
  export function describe(name: string, fn: () => void): void;
  export function expect(value: unknown): Matchers;
  export const mock: {
    module(path: string, factory: () => unknown): void;
  };
}
