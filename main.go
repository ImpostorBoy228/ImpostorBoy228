package main

import (
	"embed"
	"io/fs"
	"log"
	"net/http"
)

//go:embed static
var staticFS embed.FS

func main() {
	sub, _ := fs.Sub(staticFS, "static")

	http.Handle("/", http.FileServer(http.FS(sub)))

	log.Fatal(http.ListenAndServe(":911", nil))
}
