import { setupServer } from 'msw/node'

// Tests register their own handlers via `server.use(http.get(...))`.
// No auto-generated mocks — every test explicitly defines the routes it needs.
export const server = setupServer()
