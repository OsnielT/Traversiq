package example

type SummaryService struct{}
func (s *SummaryService) ReadSummary() string { return "Example" }

type CatalogResolver struct { service *SummaryService }
func (r *CatalogResolver) Summary() string { return r.service.ReadSummary() }
