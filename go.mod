module github.com/minio/minlz

go 1.25

require (
	github.com/klauspost/compress v1.20.1
	github.com/klauspost/cpuid/v2 v2.4.0
	github.com/klauspost/pivco v0.0.0-00010101000000-000000000000
)

require golang.org/x/sys v0.41.0 // indirect

replace github.com/klauspost/pivco => ../../klauspost/pivco
