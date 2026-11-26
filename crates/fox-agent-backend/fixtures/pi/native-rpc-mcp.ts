export default function registerTestMcp(pi: any) {
	const server = JSON.parse(process.env.FOX_PI_COMPARISON_MCP!);
	pi.registerMcpServer(server.name, {
		command: server.command,
		args: server.args,
		description: "Fox Stress Ledger and Search test connector",
		exposure: "direct",
	});
}
