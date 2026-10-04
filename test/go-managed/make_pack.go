//go:build ignore

// make_pack.go creates private native/cross fixtures. It is not a signed release maker.
package main

import (
	"archive/tar"
	"bufio"
	"compress/gzip"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
)

func main() {
	if len(os.Args) != 3 && len(os.Args) != 4 {
		fatal(errors.New("usage: go run make_pack.go <absolute-go-binary> <output-runtime.pack> [linux-aarch64]"))
	}
	goos, goarch := runtime.GOOS, runtime.GOARCH
	if len(os.Args) == 4 {
		if os.Args[3] != "linux-aarch64" || runtime.GOOS != "linux" || runtime.GOARCH != "amd64" {
			fatal(errors.New("cross fixture supports only Linux x86_64 to linux-aarch64"))
		}
		goarch = "arm64"
	}
	goBinary, err := filepath.Abs(os.Args[1])
	if err != nil {
		fatal(err)
	}
	outputPack, err := filepath.Abs(os.Args[2])
	if err != nil {
		fatal(err)
	}
	root, err := os.MkdirTemp("", "dever-go-pack-")
	if err != nil {
		fatal(err)
	}
	defer os.RemoveAll(root)
	toolDir := filepath.Join(filepath.Dir(goBinary), "..", "pkg", "tool", runtime.GOOS+"_"+runtime.GOARCH)
	goRoot := filepath.Clean(filepath.Join(filepath.Dir(goBinary), ".."))
	analyzer := filepath.Join(root, "analyze")
	build := exec.Command(goBinary, "build", "-p=1", "-o", analyzer, "./test/go-managed/analyze")
	build.Env = []string{"GOROOT=" + goRoot, "GOTOOLCHAIN=local", "GOWORK=off", "GO111MODULE=off", "CGO_ENABLED=0", "GOCACHE=" + filepath.Join(filepath.Dir(outputPack), "author-cache"), "GOMODCACHE=" + filepath.Join(root, "modules")}
	if output, err := build.CombinedOutput(); err != nil {
		fatal(fmt.Errorf("build Go source selector: %w: %s", err, output))
	}
	list := exec.Command(goBinary, "list", "-p=1", "-deps", "-export", "-f", "{{if .Export}}{{.ImportPath}} {{.Export}}{{end}}", "std")
	list.Env = append(build.Env, "GOOS="+goos, "GOARCH="+goarch)
	output, err := list.Output()
	if err != nil {
		fatal(fmt.Errorf("list fixed-target standard library: %w", err))
	}
	stdlib := make(map[string]string)
	scanner := bufio.NewScanner(strings.NewReader(string(output)))
	for scanner.Scan() {
		fields := strings.Fields(scanner.Text())
		if len(fields) == 2 {
			stdlib[fields[0]] = fields[1]
		}
	}
	if err := scanner.Err(); err != nil {
		fatal(err)
	}
	if len(stdlib) == 0 || stdlib["runtime"] == "" {
		fatal(errors.New("standard library export map is incomplete"))
	}
	files := map[string]string{
		"bin/compile": filepath.Join(toolDir, "compile"),
		"bin/link":    filepath.Join(toolDir, "link"),
		"bin/analyze": analyzer,
	}
	if goarch != runtime.GOARCH {
		tools := crossTools(goBinary, goRoot, root, goarch, build.Env)
		files["bin/compile"], files["bin/link"] = tools[0], tools[1]
	}
	var imports []string
	for path, archive := range stdlib {
		files["stdlib/"+path+".a"] = archive
		imports = append(imports, path)
	}
	sort.Strings(imports)
	var importcfg strings.Builder
	for _, path := range imports {
		fmt.Fprintf(&importcfg, "packagefile %s=stdlib/%s.a\n", path, path)
	}
	version := exec.Command(goBinary, "version")
	version.Env = []string{"GOROOT=" + goRoot, "GOTOOLCHAIN=local"}
	versionText, err := version.Output()
	if err != nil {
		fatal(err)
	}
	deverOS, deverArch := runtime.GOOS, runtime.GOARCH
	if deverOS == "darwin" {
		deverOS = "macos"
	}
	if deverArch == "amd64" {
		deverArch = "x86_64"
	} else if deverArch == "arm64" {
		deverArch = "aarch64"
	}
	target := deverOS + "-" + deverArch
	if len(os.Args) == 4 {
		target = os.Args[3]
	}
	manifest, err := json.Marshal(map[string]any{
		"format": "dever-worker-runtime-v1", "ecosystem": "go", "target": target,
		"build": map[string]any{"host": deverOS + "-" + deverArch, "compiler": "bin/compile", "linker": "bin/link", "analyzer": "bin/analyze", "stdlib_importcfg": "stdlib/importcfg", "goos": goos, "goarch": goarch, "go_version": strings.TrimSpace(string(versionText))},
	})
	if err != nil {
		fatal(err)
	}
	outputFile, err := os.Create(outputPack)
	if err != nil {
		fatal(err)
	}
	defer outputFile.Close()
	gzipWriter := gzip.NewWriter(outputFile)
	tarWriter := tar.NewWriter(gzipWriter)
	writeBytes(tarWriter, "dever-runtime.json", manifest)
	writeBytes(tarWriter, "stdlib/importcfg", []byte(importcfg.String()))
	names := make([]string, 0, len(files))
	for name := range files {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		input, err := os.Open(files[name])
		if err != nil {
			fatal(err)
		}
		info, err := input.Stat()
		if err != nil {
			fatal(err)
		}
		mode := int64(0644)
		if strings.HasPrefix(name, "bin/") {
			mode = 0755
		}
		if err := tarWriter.WriteHeader(&tar.Header{Name: name, Size: info.Size(), Mode: mode, Typeflag: tar.TypeReg}); err != nil {
			fatal(err)
		}
		if _, err := io.Copy(tarWriter, input); err != nil {
			fatal(err)
		}
		input.Close()
	}
	if err := tarWriter.Close(); err != nil {
		fatal(err)
	}
	if err := gzipWriter.Close(); err != nil {
		fatal(err)
	}
}

// The product clears the environment. Bake the output architecture into private
// host tools through an overlay; never edit the author's installed Go SDK.
func crossTools(goBinary, goRoot, root, architecture string, environment []string) []string {
	original := filepath.Join(goRoot, "src", "internal", "buildcfg", "zbootstrap.go")
	contents, err := os.ReadFile(original)
	if err != nil {
		fatal(err)
	}
	const declaration = "const defaultGOARCH = runtime.GOARCH"
	if strings.Count(string(contents), declaration) != 1 {
		fatal(errors.New("Go SDK default architecture declaration differs"))
	}
	override := filepath.Join(root, "zbootstrap.go")
	updated := strings.Replace(string(contents), declaration, "const defaultGOARCH = "+fmt.Sprintf("%q", architecture), 1)
	if err := os.WriteFile(override, []byte(updated), 0600); err != nil {
		fatal(err)
	}
	overlay := filepath.Join(root, "overlay.json")
	encoded, err := json.Marshal(map[string]any{"Replace": map[string]string{original: override}})
	if err != nil {
		fatal(err)
	}
	if err := os.WriteFile(overlay, encoded, 0600); err != nil {
		fatal(err)
	}
	var outputs []string
	for _, tool := range []string{"compile", "link"} {
		output := filepath.Join(root, tool)
		command := exec.Command(goBinary, "build", "-p=1", "-overlay", overlay, "-o", output, "cmd/"+tool)
		command.Env = environment
		if bytes, err := command.CombinedOutput(); err != nil {
			fatal(fmt.Errorf("build cross %s: %w: %s", tool, err, bytes))
		}
		outputs = append(outputs, output)
	}
	return outputs
}

func writeBytes(writer *tar.Writer, name string, data []byte) {
	if err := writer.WriteHeader(&tar.Header{Name: name, Size: int64(len(data)), Mode: 0644, Typeflag: tar.TypeReg}); err != nil {
		fatal(err)
	}
	if _, err := writer.Write(data); err != nil {
		fatal(err)
	}
}

func fatal(err error) {
	fmt.Fprintln(os.Stderr, err)
	os.Exit(1)
}
