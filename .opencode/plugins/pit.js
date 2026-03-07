// src/plugin/pit.ts
import * as net from "node:net";
import { randomUUID } from "node:crypto";
async function sendToDaemon(socketPath, method, params) {
  return new Promise((resolve) => {
    const socket = net.createConnection(socketPath);
    const request = {
      id: randomUUID(),
      method,
      params
    };
    socket.on("connect", () => {
      socket.write(JSON.stringify(request) + "\n", () => {
        socket.destroy();
        resolve();
      });
    });
    socket.on("error", (err) => {
      console.error(`[pit plugin] Socket error (${method}):`, err.message);
      socket.destroy();
      resolve();
    });
  });
}
var PitPlugin = async () => {
  const epic = process.env.PIT_EPIC;
  const socketPath = process.env.PIT_SOCKET_PATH;
  if (!epic || !socketPath) {
    return {};
  }
  return {
    event: async ({ event }) => {
      try {
        if (event.type === "session.idle") {
          await sendToDaemon(socketPath, "agent-idle", { epicId: epic });
        }
      } catch (err) {
        console.error("[pit plugin] Error handling session.idle:", err);
      }
    },
    "permission.ask": async ({ input }) => {
      try {
        await sendToDaemon(socketPath, "agent-permission", {
          epicId: epic,
          tool: input.type ?? "unknown",
          input: JSON.stringify(input)
        });
      } catch (err) {
        console.error("[pit plugin] Error handling permission.ask:", err);
      }
    }
  };
};
export {
  PitPlugin
};
