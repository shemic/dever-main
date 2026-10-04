// No package entry modules are executed. Probe exactly one immutable addon in
// an isolated runtime-only process, so addon globals cannot affect its peers.
try {
  require(process.argv[2]);
  process.exit(0);
} catch (error) {
  process.stderr.write(String(error.stack || error) + '\n');
  process.exit(10);
}
